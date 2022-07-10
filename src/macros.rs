

#[macro_export]
macro_rules! stderr_result {
    () => (io::stderr().write_all(&[0; 0]).await);
    ($($arg:tt)*) => ({
        #[allow(unused_imports)]
        use tokio::io::AsyncWriteExt;
        tokio::io::stderr().write_all(&std::format!($($arg)*).as_bytes()).await
    })
}

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


pub struct HexSlice<'a>(&'a [u8]);

impl<'a> HexSlice<'a> {
    pub fn new<T>(data: &'a T) -> HexSlice<'a>
    where
        T: ?Sized + AsRef<[u8]> + 'a,
    {
        HexSlice(data.as_ref())
    }
}

// You can choose to implement multiple traits, like Lower and UpperHex
impl fmt::Display for HexSlice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            // Decide if you want to pad the value or have spaces inbetween, etc.
            write!(f, "{:02X}", byte)?;
        }
        Ok(())
    }
}