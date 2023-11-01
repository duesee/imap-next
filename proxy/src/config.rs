use std::{
    fmt::{Display, Formatter},
    path::Path,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const fn default_imap_port() -> u16 {
    143
}

const fn default_imaps_port() -> u16 {
    993
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Config {
    pub services: Vec<Service>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Service {
    /// Name of service, e.g., "Best Email Provider".
    pub name: String,
    /// How to accept client connections?
    pub bind: Bind,
    /// How to establish server connections?
    pub connect: Connect,
}

/// How to accept client connections?
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "encryption")]
pub enum Bind {
    /// Accept non-encrypted connections from client (insecure).
    Insecure {
        /// Host.
        host: String,
        /// Port.
        #[serde(default = "default_imap_port")]
        port: u16,
    },
    /// Accept TLS-encrypted connections from client.
    Tls {
        /// Host.
        host: String,
        /// Port.
        #[serde(default = "default_imaps_port")]
        port: u16,
        /// Cryptographic objects required to accept a TLS connection.
        identity: Identity,
    },
}

impl Bind {
    /// Creates a `host:port` `String`.
    pub fn addr_port(&self) -> String {
        match self {
            Self::Tls { host, port, .. } | Self::Insecure { host, port } => {
                format!("{host}:{port}")
            }
        }
    }
}

impl Display for Bind {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            Bind::Tls { host, port, .. } => {
                write!(f, "imaps://{}:{} (TLS)", host, port)
            }
            Bind::Insecure { host, port } => {
                write!(f, "imap://{}:{} (insecure)", host, port)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum Identity {
    /// Certificate chain and leaf key.
    CertificateChainAndLeafKey {
        /// Path to certificate chain (in PEM format).
        certificate_chain_path: String,
        /// Path to leaf key (in PEM format).
        leaf_key_path: String,
    },
    // TODO(#57)
    // /// PKCS #12 bundle.
    // Pkcs12 {
    //     /// Path to PKCS #12 bundle containing certificate chain and leaf key.
    //     path: String,
    //     /// (Optional) passphrase to unlock the PKCS #12 bundle.
    //     passphrase: Option<String>,
    // },
}

/// How to establish server connections?
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "encryption")]
pub enum Connect {
    /// Establish non-encrypted connection to server (insecure).
    Insecure {
        /// Host.
        host: String,
        /// Port.
        #[serde(default = "default_imap_port")]
        port: u16,
    },
    /// Establish TLS-encrypted connection to server.
    Tls {
        /// Host.
        host: String,
        /// Port.
        #[serde(default = "default_imaps_port")]
        port: u16,
    },
}

impl Connect {
    /// Creates a `host:port` `String`.
    pub fn addr_port(&self) -> String {
        match self {
            Self::Tls { host, port, .. } | Self::Insecure { host, port } => {
                format!("{host}:{port}")
            }
        }
    }
}

impl Display for Connect {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            Connect::Tls { host, port, .. } => {
                write!(f, "imaps://{}:{} (TLS)", host, port)
            }
            Connect::Insecure { host, port } => {
                write!(f, "imap://{}:{} (insecure)", host, port)
            }
        }
    }
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
    #[error(transparent)]
    Serialize(#[from] toml::ser::Error),
}

#[cfg(test)]
mod tests {
    use crate::config::{Bind, Config, Connect, Identity, Service};

    #[test]
    fn test_config() {
        let tests = [
            (
                Config {
                    services: vec![Service {
                        name: "My Email Provider".into(),
                        bind: Bind::Insecure {
                            host: "127.0.0.1".into(),
                            port: 1143,
                        },
                        connect: Connect::Tls {
                            host: "imap.example.org".into(),
                            port: 993,
                        },
                    }],
                },
                "config.toml",
            ),
            (
                Config {
                    services: vec![Service {
                        name: "Insecure/Insecure".into(),
                        bind: Bind::Insecure {
                            host: "127.0.0.1".into(),
                            port: 1143,
                        },
                        connect: Connect::Insecure {
                            host: "127.0.0.1".into(),
                            port: 143,
                        },
                    }],
                },
                "configs/insecure_insecure.toml",
            ),
            (
                Config {
                    services: vec![Service {
                        name: "Insecure/TLS".into(),
                        bind: Bind::Insecure {
                            host: "127.0.0.1".into(),
                            port: 1143,
                        },
                        connect: Connect::Tls {
                            host: "127.0.0.1".into(),
                            port: 993,
                        },
                    }],
                },
                "configs/insecure_tls.toml",
            ),
            (
                Config {
                    services: vec![Service {
                        name: "TLS/Insecure".into(),
                        bind: Bind::Tls {
                            host: "127.0.0.1".into(),
                            port: 1993,
                            identity: Identity::CertificateChainAndLeafKey {
                                certificate_chain_path: "localhost.pem".into(),
                                leaf_key_path: "localhost-key.pem".into(),
                            },
                        },
                        connect: Connect::Insecure {
                            host: "127.0.0.1".into(),
                            port: 143,
                        },
                    }],
                },
                "configs/tls_insecure.toml",
            ),
            (
                Config {
                    services: vec![Service {
                        name: "TLS/TLS".into(),
                        bind: Bind::Tls {
                            host: "127.0.0.1".into(),
                            port: 1993,
                            identity: Identity::CertificateChainAndLeafKey {
                                certificate_chain_path: "localhost.pem".into(),
                                leaf_key_path: "localhost-key.pem".into(),
                            },
                        },
                        connect: Connect::Tls {
                            host: "127.0.0.1".into(),
                            port: 993,
                        },
                    }],
                },
                "configs/tls_tls.toml",
            ),
        ];

        for (expected, path) in tests.into_iter() {
            let got = Config::load(path).unwrap();
            assert_eq!(expected, got);
        }
    }
}
