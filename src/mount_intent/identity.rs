#[derive(Debug, PartialEq, Eq)]
pub struct RegisteredProcess {
    pub pid: i32,
    pub uid: i32,
    pub start_time: u64,
    pub package: String,
}

pub fn decode(content: &str) -> Option<RegisteredProcess> {
    fn field<'a>(content: &'a str, key: &str) -> Option<&'a str> {
        let mut values = content.lines().filter_map(|line| line.strip_prefix(key));
        let value = values.next()?;
        values.next().is_none().then_some(value)
    }
    if content.len() > 4096 {
        return None;
    }
    let pid = field(content, "pid=")?.parse().ok()?;
    let uid = field(content, "uid=")?.parse().ok()?;
    let start_time = field(content, "app_start_time=")?.parse().ok()?;
    let package = field(content, "package=")?;
    if pid <= 0
        || uid < 0
        || start_time == 0
        || package.is_empty()
        || package.len() > 255
        || !package
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(RegisteredProcess {
        pid,
        uid,
        start_time,
        package: package.to_owned(),
    })
}

impl RegisteredProcess {
    pub fn matches(&self, uid: i32, start_time: Option<u64>, package_uid: i32) -> bool {
        self.uid == uid
            && start_time == Some(self.start_time)
            && uid >= 0
            && package_uid >= 0
            && uid % 100000 == package_uid % 100000
    }
}
