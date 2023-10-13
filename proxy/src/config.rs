use std::{
    fmt::{Display, Formatter},
    path::Path,
};

use serde::Deserialize;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub services: Vec<Service>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Service {
    pub name: String,
    pub bind: Addr,
    pub connect: Addr,
}

impl Display for Addr {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        let scheme = match self.security {
            Security::Insecure => "imap",
            Security::Tls => "imaps",
        };

        write!(f, "{}://{}:{}", scheme, self.host, self.port)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Addr {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub security: Security,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub enum Security {
    #[default]
    Tls,
    Insecure,
}

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Parse(#[from] toml::de::Error),
}
