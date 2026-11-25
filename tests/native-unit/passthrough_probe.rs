use super::{probe_verified_inode, read_passthrough_probe};
use std::fs;
use std::io::{Error, Read};
use std::os::unix::fs::{MetadataExt, symlink};

#[test]
fn passthrough_probe_downgrades_only_on_verified_reopen_or_read_eio() {
    struct FailedRead(i32);
    impl Read for FailedRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(Error::from_raw_os_error(self.0))
        }
    }
    for errno in [libc::EIO, libc::EACCES] {
        for reopened in [Err(Error::from_raw_os_error(errno)), Ok(FailedRead(errno))] {
            let outcome = probe_verified_inode(reopened);
            if errno == libc::EIO {
                assert!(!outcome.unwrap());
            } else {
                assert_eq!(outcome.unwrap_err().raw_os_error(), Some(errno));
            }
        }
    }
}

#[test]
fn passthrough_probe_reads_only_a_regular_file_on_the_expected_device() {
    let root = std::env::temp_dir().join(format!("srx-probe-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let dev = fs::metadata(&root).unwrap().dev();
    let expected = format!("{}:{}", libc::major(dev), libc::minor(dev));
    let marker = root.join(".nomedia");
    let probe = |path: &std::path::Path, device: &str| {
        read_passthrough_probe(path.to_str().unwrap(), device)
    };
    assert_eq!(
        probe(&marker, &expected).unwrap_err().raw_os_error(),
        Some(libc::ENOENT)
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    fs::write(&marker, []).unwrap();
    assert!(probe(&marker, &expected).unwrap());
    fs::write(&marker, b"preserve").unwrap();
    let identity = fs::metadata(&marker).unwrap().ino();
    assert!(probe(&marker, &expected).unwrap());
    assert_eq!(fs::metadata(&marker).unwrap().ino(), identity);
    assert_eq!(fs::read(&marker).unwrap(), b"preserve");
    assert_eq!(
        probe(&marker, "wrong-device").unwrap_err().raw_os_error(),
        Some(libc::EXDEV)
    );
    assert_eq!(
        probe(&root, &expected).unwrap_err().raw_os_error(),
        Some(libc::EXDEV)
    );
    let link = root.join("link");
    symlink(&marker, &link).unwrap();
    assert_eq!(
        probe(&link, &expected).unwrap_err().raw_os_error(),
        Some(libc::EXDEV)
    );
    fs::remove_dir_all(root).unwrap();
}
