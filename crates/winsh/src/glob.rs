use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
pub(crate) fn matches(pattern: &str, value: &str) -> bool {
    fn inner(p: &[char], s: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('*') => inner(&p[1..], s) || (!s.is_empty() && inner(p, &s[1..])),
            Some('?') => !s.is_empty() && inner(&p[1..], &s[1..]),
            Some('\\') if p.len() > 1 => s.first() == p.get(1) && inner(&p[2..], &s[1..]),
            Some('[') => {
                let mut i = 1;
                let negate = matches!(p.get(i), Some('!' | '^'));
                if negate {
                    i += 1;
                }
                let mut yes = false;
                while i < p.len() && (p[i] != ']' || i == 1 + usize::from(negate)) {
                    if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
                        yes |= s.first().is_some_and(|c| *c >= p[i] && *c <= p[i + 2]);
                        i += 3;
                    } else {
                        yes |= s.first() == p.get(i);
                        i += 1;
                    }
                }
                if i == p.len() {
                    return s.first() == Some(&'[') && inner(&p[1..], &s[1..]);
                }
                !s.is_empty() && yes != negate && inner(&p[i + 1..], &s[1..])
            }
            Some(ch) => s.first() == Some(ch) && inner(&p[1..], &s[1..]),
        }
    }
    inner(
        &pattern.chars().collect::<Vec<_>>(),
        &value.chars().collect::<Vec<_>>(),
    )
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
