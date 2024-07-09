// async write to stderr
#[macro_export]
macro_rules! stderr_result {
    () => (io::stderr().write_all(&[0; 0]).await);
    ($($arg:tt)*) => ({
        #[allow(unused_imports)]
        use tokio::io::AsyncWriteExt;
        tokio::io::stderr().write_all(&std::format!($($arg)*).as_bytes()).await
    })
}

// async write to stderr
//
// with ?; result progagation
#[macro_export]
macro_rules! stderr {
    () => (io::stderr().write_all(&[0; 0]).await);
    ($($arg:tt)*) => ({
        #[allow(unused_imports)]
        use tokio::io::AsyncWriteExt;
        tokio::io::stderr().write_all(&std::format!($($arg)*).as_bytes()).await?
    })
}

use std::fmt;

pub struct HexSlice<'a>(pub &'a [u8]);



impl fmt::Display for HexSlice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{:02X}", byte)?;
        }
        Ok(())
    }
}
