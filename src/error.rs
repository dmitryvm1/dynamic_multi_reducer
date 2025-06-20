use std::net::AddrParseError;

use thiserror::Error;
#[derive(Error, Debug)]
pub enum Error {
    #[error("Error parsing ip address.")]
    ParseIPAddress(#[from] AddrParseError),
    #[error("IO error")]
    IO(#[from] std::io::Error),
    #[error("Fast socks error")]
    FastSocks(#[from] fast_socks5::SocksError)

}