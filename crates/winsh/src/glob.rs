use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
pub(crate) fn matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<_> = pattern.chars().collect();
    let value: Vec<_> = value.chars().collect();
    let (mut p, mut v) = (0, 0);
    // A case subject can be much longer than a filename. Retry the last star
    // with one more character without consuming a stack frame per character.
    let mut star = None;
    while v < value.len() {
        if pattern.get(p) == Some(&'*') {
            p += 1;
            star = Some((p, v));
            continue;
        }
        let (matched, width) = match pattern.get(p) {
            Some('?') => (true, 1),
            Some('[') => bracket(&pattern[p..], value[v]).unwrap_or((value[v] == '[', 1)),
            Some('\\') if p + 1 < pattern.len() => (value[v] == pattern[p + 1], 2),
            literal => (literal == Some(&value[v]), 1),
        };
        if matched {
            p += width;
            v += 1;
        } else if let Some((resume, consumed)) = &mut star {
            *consumed += 1;
            v = *consumed;
            p = *resume;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|ch| *ch == '*')
}

fn bracket(pattern: &[char], value: char) -> Option<(bool, usize)> {
    fn member(pattern: &[char], i: &mut usize) -> Option<char> {
        if pattern.get(*i) == Some(&'\\') {
            *i += 1;
        }
        let ch = *pattern.get(*i)?;
        *i += 1;
        Some(ch)
    }
    let negate = matches!(pattern.get(1), Some('!' | '^'));
    let mut i = 1 + usize::from(negate);
    let mut matched = false;
    loop {
        if pattern[i..].starts_with(&['[', ':']) {
            let name = &pattern[i + 2..];
            let end = name.windows(2).position(|pair| pair == [':', ']'])?;
            let name: String = name[..end].iter().collect();
            matched |= match name.as_str() {
                "alnum" => value.is_ascii_alphanumeric(),
                "alpha" => value.is_ascii_alphabetic(),
                "blank" => matches!(value, ' ' | '\t'),
                "cntrl" => value.is_ascii_control(),
                "digit" => value.is_ascii_digit(),
                "graph" => value.is_ascii_graphic(),
                "lower" => value.is_ascii_lowercase(),
                "print" => value == ' ' || value.is_ascii_graphic(),
                "punct" => value.is_ascii_punctuation(),
                "space" => value.is_ascii_whitespace() || value == '\x0b',
                "upper" => value.is_ascii_uppercase(),
                "xdigit" => value.is_ascii_hexdigit(),
                _ => return None,
            };
            i += end + 4;
        } else {
            let first = member(pattern, &mut i)?;
            if pattern.get(i) == Some(&'-') && pattern.get(i + 1) != Some(&']') {
                i += 1;
                let last = member(pattern, &mut i)?;
                matched |= (first..=last).contains(&value);
            } else {
                matched |= value == first;
            }
        }
        if pattern.get(i) == Some(&']') {
            return Some((matched != negate, i + 1));
        }
    }
}
pub(crate) fn paths(cwd: &Path, pattern: &str) -> Vec<OsString> {
    let mut candidates = vec![PathBuf::new()];
    for component in pattern.split('/') {
        if component.is_empty() {
            if candidates == [PathBuf::new()] {
                candidates[0].push("/");
            }
            continue;
        }
        let wildcard = component.contains(['*', '?', '[']);
        let mut next = Vec::new();
        for base in candidates {
            if wildcard {
                let Ok(entries) = std::fs::read_dir(cwd.join(&base)) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let text = name.to_string_lossy();
                    if text.starts_with('.') && !component.starts_with('.') {
                        continue;
                    }
                    if matches(component, &text) {
                        next.push(base.join(name));
                    }
                }
            } else {
                let path = base.join(unescape(component));
                if cwd.join(&path).exists() {
                    next.push(path);
                }
            }
        }
        candidates = next;
    }
    let trailing_slash = pattern.ends_with('/');
    let mut result: Vec<_> = candidates
        .into_iter()
        .filter(|p| !trailing_slash || cwd.join(p).is_dir())
        .map(|p| {
            let mut name = p.into_os_string();
            if trailing_slash {
                name.push("/");
            }
            OsString::from(name.to_string_lossy().replace('\\', "/"))
        })
        .collect();
    result.sort();
    result
}
fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(ch) = chars.next() {
                out.push(ch);
            }
        } else {
            out.push(ch);
        }
    }
    out
}
