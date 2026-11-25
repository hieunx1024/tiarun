pub fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

pub(crate) fn expand_tilde(path_str: &str) -> String {
    if path_str.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return path_str.replacen("~/", &format!("{}/", home.to_string_lossy()), 1);
        }
    } else if path_str.starts_with(r"~\") {
        if let Some(home) = dirs::home_dir() {
            return path_str.replacen(r"~\", &format!("{}\\", home.to_string_lossy()), 1);
        }
    } else if path_str == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    }
    path_str.to_string()
}

/// Minimal percent-encoding for a URL query component (RFC 3986 "unreserved" set
/// passed through as-is, everything else percent-encoded byte-by-byte, so UTF-8
/// text like Vietnamese input is encoded correctly). No external crate needed for
/// something this small.
pub(crate) fn url_encode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for &byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// Helper function to strip Vietnamese accents and convert to lowercase for accent-insensitive search.
pub fn remove_vietnamese_accents(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'ả' | 'ã' | 'ạ' | 'ă' | 'ắ' | 'ằ' | 'ẳ' | 'ẵ' | 'ặ' | 'â' | 'ấ' | 'ầ' | 'ẩ' | 'ẫ' | 'ậ'
            | 'Á' | 'À' | 'Ả' | 'Ã' | 'Ạ' | 'Ă' | 'Ắ' | 'Ằ' | 'Ẳ' | 'Ẵ' | 'Ặ' | 'Â' | 'Ấ' | 'Ầ' | 'Ẩ' | 'Ẫ' | 'Ậ' => 'a',
            'é' | 'è' | 'ẻ' | 'ẽ' | 'ẹ' | 'ê' | 'ế' | 'ề' | 'ể' | 'ễ' | 'ệ'
            | 'É' | 'È' | 'Ẻ' | 'Ẽ' | 'Ẹ' | 'Ê' | 'Ế' | 'Ề' | 'Ể' | 'Ễ' | 'Ệ' => 'e',
            'í' | 'ì' | 'ỉ' | 'ĩ' | 'ị' | 'Í' | 'Ì' | 'Ỉ' | 'Ĩ' | 'Ị' => 'i',
            'ó' | 'ò' | 'ỏ' | 'õ' | 'ọ' | 'ô' | 'ố' | 'ồ' | 'ổ' | 'ỗ' | 'ộ' | 'ơ' | 'ớ' | 'ờ' | 'ở' | 'ỡ' | 'ợ'
            | 'Ó' | 'Ò' | 'Ỏ' | 'Õ' | 'Ọ' | 'Ô' | 'Ố' | 'Ồ' | 'Ổ' | 'Ỗ' | 'Ộ' | 'Ơ' | 'Ớ' | 'Ờ' | 'Ở' | 'Ỡ' | 'Ợ' => 'o',
            'ú' | 'ù' | 'ủ' | 'ũ' | 'ụ' | 'ư' | 'ứ' | 'ừ' | 'ử' | 'ữ' | 'ự'
            | 'Ú' | 'Ù' | 'Ủ' | 'Ũ' | 'Ụ' | 'Ư' | 'Ứ' | 'Ừ' | 'Ử' | 'Ữ' | 'Ự' => 'u',
            'ý' | 'ỳ' | 'ỷ' | 'ỹ' | 'ỵ' | 'Ý' | 'Ỳ' | 'Ỷ' | 'Ỹ' | 'Ỵ' => 'y',
            'đ' | 'Đ' => 'd',
            _ => c.to_ascii_lowercase(),
        })
        .collect()
}

pub fn strip_vendor_prefix(name: &str) -> &str {
    for prefix in &["microsoft ", "google ", "mozilla ", "the "] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest;
        }
    }
    name
}

pub fn quote_arg(s: &str) -> String {
    if s.contains(' ') || s.contains('\t') || s.contains('"') || s.contains('\'') {
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

pub fn parse_command_line(cmd: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = None;
    let mut in_arg = false;
    let mut chars = cmd.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    in_arg = true;
                }
            }
            '"' | '\'' => {
                if let Some(q) = in_quotes {
                    if q == c {
                        in_quotes = None;
                    } else {
                        current.push(c);
                    }
                } else {
                    in_quotes = Some(c);
                    in_arg = true;
                }
            }
            c if c.is_whitespace() => {
                if in_quotes.is_some() {
                    current.push(c);
                } else if in_arg {
                    args.push(current);
                    current = String::new();
                    in_arg = false;
                }
            }
            c => {
                current.push(c);
                in_arg = true;
            }
        }
    }

    if in_arg {
        args.push(current);
    }

    args
}

/// Extracts a clean, user-friendly executable path or name from a raw exec command.
/// Strips out `env`, environment variables (`VAR=val`), placeholders (`%U`, `%f`),
/// and unwraps `flatpak run` commands.
pub fn clean_app_exec(raw: &str) -> String {
    let tokens = parse_command_line(raw);
    let mut meaningful = Vec::new();
    let mut in_env = false;
    for (i, t) in tokens.iter().enumerate() {
        if i == 0 && t == "env" {
            in_env = true;
            continue;
        }
        if in_env && t.contains('=') {
            continue;
        }
        in_env = false;
        if t.starts_with('%') || t == "--" {
            continue;
        }
        meaningful.push(t.as_str());
    }

    if meaningful.is_empty() {
        return raw.to_string();
    }

    // Flatpak app ID formatting
    if meaningful.len() >= 3 && meaningful[0] == "flatpak" && meaningful[1] == "run" {
        return format!("flatpak: {}", meaningful[2]);
    }

    meaningful[0].to_string()
}
