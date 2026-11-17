use std::borrow::Cow;

/// 监视记录以 `|` 分字段、换行分记录；百分号先转义，保证文件名可无损还原。
pub fn encode(value: &str) -> Cow<'_, str> {
    if !value
        .bytes()
        .any(|byte| matches!(byte, b'%' | b'|' | b'\r' | b'\n'))
    {
        return Cow::Borrowed(value);
    }
    Cow::Owned(
        value
            .replace('%', "%25")
            .replace('|', "%7C")
            .replace('\r', "%0D")
            .replace('\n', "%0A"),
    )
}
