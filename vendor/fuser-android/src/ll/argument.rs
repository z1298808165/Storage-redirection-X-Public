//! Argument decomposition for FUSE operation requests.
//!
//! Helper to decompose a slice of binary data (incoming FUSE request) into multiple data
//! structures (request arguments).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

use zerocopy::FromBytes;
use zerocopy::Immutable;
use zerocopy::KnownLayout;
use zerocopy::error::ConvertError;

/// An iterator that can be used to fetch typed arguments from a byte slice.
pub(crate) struct ArgumentIterator<'a> {
    data: &'a [u8],
}

impl<'a> ArgumentIterator<'a> {
    /// Create a new argument iterator for the given byte slice.
    pub(crate) fn new(data: &'a [u8]) -> ArgumentIterator<'a> {
        ArgumentIterator { data }
    }

    /// Returns the size of the remaining data.
    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }

    /// Fetch a slice of all remaining bytes.
    pub(crate) fn fetch_all(&mut self) -> &'a [u8] {
        let bytes = self.data;
        self.data = &[];
        bytes
    }

    /// Fetch a typed argument. Returns `None` if there's not enough data left.
    pub(crate) fn fetch<T: FromBytes + KnownLayout + Immutable>(&mut self) -> Option<&'a T> {
        match zerocopy::Ref::<_, T>::from_prefix(self.data) {
            Err(ConvertError::Alignment(_)) => {
                // Panic on alignment errors as this is under the control
                // of the programmer, we can still return None for size
                // failures as this may be caused by insufficient external
                // data.
                panic!("Data unaligned");
            }
            Err(ConvertError::Size(_)) => None,
            Err(ConvertError::Validity(infallible)) => match infallible {},
            Ok((x, rest)) => {
                self.data = rest;
                Some(zerocopy::Ref::<&[u8], T>::into_ref(x))
            }
        }
    }

    /// Fetch a slice of typed of arguments. Returns `None` if there's not enough data left.
    pub(crate) fn fetch_slice<T: FromBytes + Immutable>(
        &mut self,
        count: usize,
    ) -> Option<&'a [T]> {
        match zerocopy::Ref::<_, [T]>::from_prefix_with_elems(self.data, count) {
            Err(ConvertError::Alignment(_)) => {
                // Panic on alignment errors as this is under the control
                // of the programmer, we can still return None for size
                // failures as this may be caused by insufficient external
                // data.
                panic!("Data unaligned");
            }
            Err(ConvertError::Size(_)) => None,
            Err(ConvertError::Validity(infallible)) => match infallible {},
            Ok((x, rest)) => {
                self.data = rest;
                Some(zerocopy::Ref::<&[u8], [T]>::into_ref(x))
            }
        }
    }

    /// Fetch a (zero-terminated) string (can be non-utf8). Returns `None` if there's not enough
    /// data left or no zero-termination could be found.
    pub(crate) fn fetch_str(&mut self) -> Option<&'a OsStr> {
        let len = memchr::memchr(0, self.data)?;
        let (out, rest) = self.data.split_at(len);
        self.data = &rest[1..];
        Some(OsStr::from_bytes(out))
    }
}
