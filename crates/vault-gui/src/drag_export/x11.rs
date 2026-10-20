use std::sync::Mutex;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

mod source;
static X11_TEST_LOCK: Mutex<()> = Mutex::new(());
