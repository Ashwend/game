use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

use anyhow::{Result, bail};

use crate::app::state::DirectConnectDialog;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DirectConnectTarget {
    pub(super) host: String,
    pub(super) port: u16,
}

pub(super) fn direct_connect_target(dialog: &DirectConnectDialog) -> Result<DirectConnectTarget> {
    let host_input = dialog.host.trim();
    if let Ok(addr) = host_input.parse::<SocketAddr>() {
        return Ok(DirectConnectTarget {
            host: addr.ip().to_string(),
            port: addr.port(),
        });
    }

    let (host, port_input) =
        split_inline_host_port(host_input).unwrap_or((host_input, dialog.port.trim()));
    if host.is_empty() {
        bail!("Server address is required.");
    }

    let Ok(port) = port_input.parse::<u16>() else {
        bail!("Port must be a number between 1 and 65535.");
    };
    if port == 0 {
        bail!("Port must be a number between 1 and 65535.");
    }

    Ok(DirectConnectTarget {
        host: host.trim_matches(['[', ']']).to_owned(),
        port,
    })
}

pub(super) fn resolve_direct_connect_target(target: &DirectConnectTarget) -> Result<SocketAddr> {
    let host = target.host.trim();
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, target.port));
    }

    (host, target.port)
        .to_socket_addrs()
        .map_err(|_| anyhow::anyhow!("Could not resolve server address."))?
        .next()
        .ok_or_else(|| anyhow::anyhow!("Could not resolve server address."))
}

fn split_inline_host_port(host_input: &str) -> Option<(&str, &str)> {
    if let Some(bracketed) = host_input.strip_prefix('[') {
        let (host, port) = bracketed.rsplit_once("]:")?;
        return Some((host, port));
    }

    if host_input.matches(':').count() == 1 {
        return host_input.rsplit_once(':');
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(host: &str, port: &str) -> DirectConnectDialog {
        DirectConnectDialog {
            host: host.to_owned(),
            port: port.to_owned(),
            error: None,
            attempt: None,
        }
    }

    #[test]
    fn direct_connect_target_parses_ip_host_and_port() {
        let dialog = dialog("127.0.0.1", "7777");

        assert_eq!(
            direct_connect_target(&dialog).expect("target should parse"),
            DirectConnectTarget {
                host: "127.0.0.1".to_owned(),
                port: 7777,
            }
        );
    }

    #[test]
    fn direct_connect_target_accepts_pasted_host_and_port() {
        let dialog = dialog("127.0.0.1:8888", "7777");

        assert_eq!(
            direct_connect_target(&dialog).expect("target should parse"),
            DirectConnectTarget {
                host: "127.0.0.1".to_owned(),
                port: 8888,
            }
        );
    }

    #[test]
    fn direct_connect_target_rejects_empty_host_and_invalid_port() {
        assert!(direct_connect_target(&dialog(" ", "7777")).is_err());
        assert!(direct_connect_target(&dialog("127.0.0.1", "0")).is_err());
    }

    #[test]
    fn resolve_direct_connect_target_handles_ip_without_dns() {
        let target = DirectConnectTarget {
            host: "127.0.0.1".to_owned(),
            port: 7777,
        };

        assert_eq!(
            resolve_direct_connect_target(&target).expect("target should resolve"),
            SocketAddr::from(([127, 0, 0, 1], 7777))
        );
    }

    #[test]
    fn direct_connect_target_uses_separate_port_field_for_bare_hostname() {
        // A hostname with no inline port pulls the port from the field.
        let target = direct_connect_target(&dialog("example.com", "9999")).expect("parses");
        assert_eq!(
            target,
            DirectConnectTarget {
                host: "example.com".to_owned(),
                port: 9999,
            }
        );
    }

    #[test]
    fn direct_connect_target_trims_surrounding_whitespace() {
        let target = direct_connect_target(&dialog("  example.com  ", "  7000  ")).expect("parses");
        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 7000);
    }

    #[test]
    fn direct_connect_target_parses_full_ipv6_socket_addr() {
        // A complete bracketed IPv6 socket address parses directly via the
        // SocketAddr fast-path, normalising the host to its canonical form.
        let target = direct_connect_target(&dialog("[::1]:5555", "1234")).expect("parses");
        assert_eq!(
            target,
            DirectConnectTarget {
                host: "::1".to_owned(),
                port: 5555,
            }
        );
    }

    #[test]
    fn direct_connect_target_parses_bracketed_ipv6_host_with_field_port() {
        // Bracketed host without an inline port falls through to the
        // split helper, which strips the brackets and uses the port field.
        let target = direct_connect_target(&dialog("[2001:db8::1]:8443", "1234")).expect("parses");
        assert_eq!(target.host, "2001:db8::1");
        assert_eq!(target.port, 8443);
    }

    #[test]
    fn direct_connect_target_rejects_zero_and_overflow_ports() {
        assert!(direct_connect_target(&dialog("host", "0")).is_err());
        assert!(direct_connect_target(&dialog("host", "70000")).is_err());
        assert!(direct_connect_target(&dialog("host", "")).is_err());
        assert!(direct_connect_target(&dialog("host", "notanumber")).is_err());
    }

    #[test]
    fn direct_connect_target_rejects_empty_inline_host() {
        // "host:" form has an empty host segment.
        assert!(direct_connect_target(&dialog(":7777", "1234")).is_err());
    }

    #[test]
    fn direct_connect_target_inline_port_overrides_field() {
        // When the host carries an inline `:port`, it wins over the field.
        let target = direct_connect_target(&dialog("myhost:2020", "7777")).expect("parses");
        assert_eq!(target.host, "myhost");
        assert_eq!(target.port, 2020);
    }

    #[test]
    fn split_inline_host_port_only_splits_single_colon() {
        assert_eq!(split_inline_host_port("a:1"), Some(("a", "1")));
        // Bare IPv6 (multiple colons, no brackets) is ambiguous and is not
        // split inline, it falls back to the port field.
        assert_eq!(split_inline_host_port("2001:db8::1"), None);
        assert_eq!(split_inline_host_port("plainhost"), None);
        assert_eq!(split_inline_host_port("[::1]:9"), Some(("::1", "9")));
    }

    #[test]
    fn resolve_direct_connect_target_resolves_ipv6_literal() {
        let target = DirectConnectTarget {
            host: "::1".to_owned(),
            port: 4321,
        };
        let resolved = resolve_direct_connect_target(&target).expect("resolves");
        assert_eq!(resolved.port(), 4321);
        assert!(resolved.is_ipv6());
    }
}
