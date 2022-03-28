/*!
Macros

*/

// -------------
// error-chain
// -------------

/// Helper for formatting Errors that wrap strings
macro_rules! format_err {
    ($error:expr, $str:expr) => {
        $error(format!($str))
    };
    ($error:expr, $str:expr, $($arg:expr),*) => {
        $error(format!($str, $($arg),*))
    }
}

/// Helper for formatting strings with error-chain's `bail!` macro
macro_rules! bail_fmt {
    ($error:expr, $str:expr) => {
        bail!(format_err!($error, $str))
    };
    ($error:expr, $str:expr, $($arg:expr),*) => {
        bail!(format_err!($error, $str, $($arg),*))
    }
}
