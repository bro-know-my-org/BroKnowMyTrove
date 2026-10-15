//! Development-only PTY probe; not part of the installed command surface.
use std::io::Read;

fn main() {
    let confirm = std::env::args().any(|arg| arg == "confirm");
    match vault_cli::password::read_password(confirm) {
        Ok(_) => {
            let mut data = String::new();
            std::io::stdin().read_to_string(&mut data).unwrap();
            println!("confirmed; stdin-bytes={}", data.len());
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
