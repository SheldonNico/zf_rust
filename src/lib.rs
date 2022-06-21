#![feature(io_safety)]
pub mod filter;
pub mod ui;

pub use filter::{Candidate, rank_candidates};
pub use ui::Terminal;

pub fn other_error<S: Into<String>>(simple_msg: S) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, simple_msg.into())
}

