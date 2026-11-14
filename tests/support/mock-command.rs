use std::io::{self, Read, Write};
fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    let first = std::env::args().nth(1).unwrap_or_default();
    writeln!(io::stdout(), "out:{first}:{input}").unwrap();
    writeln!(io::stderr(), "diagnostic").unwrap();
    std::process::exit(std::env::var("MOCK_EXIT_CODE").ok()
        .and_then(|v| v.parse().ok()).unwrap_or(0));
}
