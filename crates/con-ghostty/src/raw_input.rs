//! Byte-preserving automation input, separate from keyboard and paste handling.
use crate::GhosttyTerminal;

impl GhosttyTerminal {
    /// Queue one raw PTY write through Ghostty's native `text:` action.
    /// Unlike surface_text (paste) or surface_key (keyboard), this action
    /// decodes Zig byte escapes and queues a single termio Message.writeReq.
    /// Success means queued, not that the child consumed or submitted it.
    pub fn write_raw_to_pty(&self, data: &[u8]) -> Result<bool, String> {
        if data.is_empty() {
            return Ok(true);
        }
        self.perform_binding_action(&raw_action(data))
    }
}

fn raw_action(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut action = String::with_capacity(5 + data.len() * 4);
    action.push_str("text:");
    for &byte in data {
        action.push_str("\\x");
        action.push(HEX[usize::from(byte >> 4)] as char);
        action.push(HEX[usize::from(byte & 15)] as char);
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    // Mock the pinned Ghostty text-action decoder and PTY sink. The native
    // action contract is config/string.zig parse -> one Message.writeReq;
    // no key event or paste wrapper is involved.
    fn mock_binding_write(action: &str, pty: &mut impl Write) {
        let encoded = action.strip_prefix("text:").unwrap();
        let bytes: Vec<u8> = encoded
            .as_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| {
                assert_eq!(&chunk[..2], b"\\x");
                u8::from_str_radix(std::str::from_utf8(&chunk[2..]).unwrap(), 16).unwrap()
            })
            .collect();
        pty.write_all(&bytes).unwrap();
    }

    #[test]
    fn raw_pty_sink_receives_contiguous_bracketed_paste_and_all_bytes() {
        let mut payload = b"\x1b[200~Read context\x1b[201~\n".to_vec();
        payload.extend(0..=255); // Includes NUL, backslash, CR and non-UTF8.
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        mock_binding_write(&raw_action(&payload), &mut writer);
        let mut received = vec![0; payload.len()];
        reader.read_exact(&mut received).unwrap();
        assert_eq!(received, payload);
        assert!(received.starts_with(b"\x1b[200~"));
    }
}
