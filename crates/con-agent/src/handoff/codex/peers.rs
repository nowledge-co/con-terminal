//! Match both halves of a loopback TCP connection, never a listening port alone.
use std::{collections::BTreeSet, net::SocketAddr, path::Path};

use anyhow::{Context, Result, ensure};

use super::lsof;
use crate::handoff::binding::macos_process_args;

pub(super) async fn codex_app_server_peer_pids(foreground_tui_pid: i32) -> Result<Vec<i32>> {
    let output = lsof::inspect(&["-nP", "-iTCP", "-sTCP:ESTABLISHED", "-FpcfnT"]).await?;
    let mut peers = Vec::new();
    for pid in parse_peer_candidates(foreground_tui_pid, &output)? {
        let args = macos_process_args(pid).context("Cannot inspect Codex app-server peer")?;
        if is_app_server(&args) {
            peers.push(pid);
        }
    }
    Ok(peers)
}

pub(super) fn parse_peer_candidates(tui: i32, output: &[u8]) -> Result<Vec<i32>> {
    peer_candidates(tui, &parse_connections(output)?)
}

fn is_app_server(args: &[String]) -> bool {
    args.first()
        .and_then(|s| Path::new(s).file_name())
        .is_some_and(|s| s == "codex")
        && args.get(1).is_some_and(|s| s == "app-server")
}

#[derive(Clone, Debug)]
struct Connection {
    pid: i32,
    codex: bool,
    local: SocketAddr,
    remote: SocketAddr,
    established: bool,
}

fn parse_connections(output: &[u8]) -> Result<Vec<Connection>> {
    let text = std::str::from_utf8(output).context("Invalid Codex socket inspection output")?;
    let (mut pid, mut codex, mut current) = (None, false, None);
    let mut sockets: Vec<Connection> = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = Some(value.parse::<i32>().context("Invalid socket owner PID")?);
            codex = false;
            current = None;
        } else if let Some(command) = line.strip_prefix('c') {
            codex = command == "codex";
        } else if line.starts_with('f') {
            current = None;
        } else if let Some(name) = line.strip_prefix('n') {
            current = None;
            let Some((local, remote)) = name.split_once("->") else {
                continue;
            };
            // Non-loopback IPv6 may contain interface zone IDs. Those cannot
            // belong to this protocol; numeric loopback addresses parse here.
            let (Ok(local), Ok(remote)) =
                (local.parse::<SocketAddr>(), remote.parse::<SocketAddr>())
            else {
                continue;
            };
            if !local.ip().is_loopback() || !remote.ip().is_loopback() {
                continue;
            }
            sockets.push(Connection {
                pid: pid.context("Missing socket owner PID")?,
                codex,
                local,
                remote,
                established: false,
            });
            current = Some(sockets.len() - 1);
        } else if line == "TST=ESTABLISHED"
            && let Some(index) = current
        {
            sockets[index].established = true;
        }
    }
    Ok(sockets)
}

fn peer_candidates(tui: i32, sockets: &[Connection]) -> Result<Vec<i32>> {
    let mut peers = BTreeSet::new();
    for socket in sockets.iter().filter(|s| s.pid == tui && s.established) {
        let reverse: BTreeSet<_> = sockets
            .iter()
            .filter(|other| {
                other.pid != tui
                    && other.established
                    && other.local == socket.remote
                    && other.remote == socket.local
            })
            .map(|s| s.pid)
            .collect();
        ensure!(reverse.len() <= 1, "Codex socket has multiple peer owners");
        peers.extend(
            sockets
                .iter()
                .filter(|s| reverse.contains(&s.pid) && s.codex)
                .map(|s| s.pid),
        );
    }
    Ok(peers.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socket(pid: i32, local: &str, remote: &str) -> String {
        format!("p{pid}\nccodex\nf9\nn{local}->{remote}\nTST=ESTABLISHED\n")
    }

    #[test]
    fn exact_reverse_pair_separates_tuis_sharing_a_listener() {
        let output = [
            socket(10, "127.0.0.1:5000", "127.0.0.1:6000"),
            socket(20, "127.0.0.1:6000", "127.0.0.1:5000"),
            socket(11, "127.0.0.1:5000", "127.0.0.1:6001"),
            socket(21, "127.0.0.1:6001", "127.0.0.1:5000"),
            socket(20, "127.0.0.1:6000", "127.0.0.1:5000"),
        ]
        .concat();
        let sockets = parse_connections(output.as_bytes()).unwrap();
        assert_eq!(peer_candidates(10, &sockets).unwrap(), [20]);
        assert_eq!(peer_candidates(11, &sockets).unwrap(), [21]);
    }

    #[test]
    fn ipv6_supported_but_listening_remote_and_unpaired_sockets_are_not_peers() {
        let output = [
            socket(10, "[::1]:5000", "[::1]:6000"),
            socket(20, "[::1]:6000", "[::1]:5000"),
            socket(10, "127.0.0.1:5001", "1.2.3.4:6001"),
            socket(21, "1.2.3.4:6001", "127.0.0.1:5001"),
            socket(10, "127.0.0.1:5002", "127.0.0.1:6002"),
            socket(22, "127.0.0.1:6002", "127.0.0.1:5002").replace("ESTABLISHED", "SYN_SENT"),
            "p10\nccodex\nf10\nn127.0.0.1:5003\nTST=LISTEN\n".into(),
        ]
        .concat();
        assert_eq!(
            peer_candidates(10, &parse_connections(output.as_bytes()).unwrap()).unwrap(),
            [20]
        );
        assert!(peer_candidates(99, &[]).unwrap().is_empty());
    }

    #[test]
    fn reused_port_without_reverse_tuple_and_non_codex_proxy_are_ignored() {
        let output = [
            socket(10, "127.0.0.1:5000", "127.0.0.1:6000"),
            socket(20, "127.0.0.1:6000", "127.0.0.1:5001"),
            socket(21, "127.0.0.1:6000", "127.0.0.1:5000").replace("ccodex", "cproxy"),
        ]
        .concat();
        assert!(
            peer_candidates(10, &parse_connections(output.as_bytes()).unwrap())
                .unwrap()
                .is_empty()
        );
        assert!(!is_app_server(&[
            "codex".into(),
            "exec".into(),
            "app-server".into()
        ]));
        assert!(is_app_server(&[
            "/opt/bin/codex".into(),
            "app-server".into()
        ]));
    }

    #[test]
    fn multiple_owners_fail_closed() {
        let output = [
            socket(10, "127.0.0.1:5000", "127.0.0.1:6000"),
            socket(20, "127.0.0.1:6000", "127.0.0.1:5000"),
            socket(21, "127.0.0.1:6000", "127.0.0.1:5000"),
        ]
        .concat();
        assert!(peer_candidates(10, &parse_connections(output.as_bytes()).unwrap()).is_err());
    }
}
