//! ANSI escape sequence handling.
//!
//! Parsers are frequently fed colorized CLI output. Stripping ANSI codes
//! before pattern-matching keeps the rest of the parser simple.

/// Strips ANSI escape sequences from a string.
///
/// Handles CSI sequences (`\x1b[...m`), OSC sequences (`\x1b]...\x07` or
/// `\x1b]...\x1b\\`), and simple escape sequences (ESC followed by a single
/// char).
pub(super) fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == 0x1b {
            // ESC character - start of escape sequence
            i += 1;
            if i >= bytes.len() {
                break;
            }

            match bytes[i] {
                b'[' => {
                    // CSI sequence: ESC [ ... (final byte in 0x40-0x7E range)
                    i += 1;
                    while i < bytes.len() && !(0x40..=0x7E).contains(&bytes[i]) {
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1; // Skip final byte
                    }
                }
                b']' => {
                    // OSC sequence: ESC ] ... (terminated by BEL or ST)
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Simple escape sequence: ESC + single char
                    i += 1;
                }
            }
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8_lossy(&result).into_owned()
}
