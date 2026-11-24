use crate::Shell;
pub(crate) fn evaluate(source: &str, shell: &Shell) -> Result<i64, String> {
    if source.contains("++") || source.contains("--") || source.contains("**") {
        return Err("arithmetic increment and exponentiation are not supported".into());
    }
    if source.trim().is_empty() {
        return Ok(0);
    }
    let mut parser = Parser {
        source,
        pos: 0,
        shell,
        depth: 0,
    };
    let value = parser.expression(0, true)?;
    parser.space();
    if parser.pos != source.len() {
        return Err("unsupported arithmetic expression".into());
    }
    Ok(value)
}
struct Parser<'a> {
    source: &'a str,
    pos: usize,
    shell: &'a Shell,
    depth: usize,
}
impl Parser<'_> {
    fn space(&mut self) {
        while self
            .source
            .as_bytes()
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }
    fn expression(&mut self, minimum: u8, active: bool) -> Result<i64, String> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("arithmetic nesting limit (64)".into());
        }
        self.space();
        let rest = &self.source[self.pos..];
        let mut left = if let Some(op) = rest
            .chars()
            .next()
            .filter(|c| matches!(c, '+' | '-' | '!' | '~'))
        {
            self.pos += 1;
            let value = self.expression(11, active)?;
            match op {
                '-' => value.wrapping_neg(),
                '!' => i64::from(value == 0),
                '~' => !value,
                _ => value,
            }
        } else if rest.starts_with('(') {
            self.pos += 1;
            let value = self.expression(0, active)?;
            self.space();
            if !self.source[self.pos..].starts_with(')') {
                return Err("expected ) in arithmetic expression".into());
            }
            self.pos += 1;
            value
        } else {
            let start = self.pos;
            while self
                .source
                .as_bytes()
                .get(self.pos)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                self.pos += 1;
            }
            let token = &self.source[start..self.pos];
            if token.is_empty() {
                return Err("expected integer or variable in arithmetic expression".into());
            }
            if crate::syntax::valid_name(token) {
                let value = self.shell.var(token).unwrap_or_default().to_string_lossy();
                if value.is_empty() || !active {
                    0
                } else {
                    integer(&value)?
                }
            } else {
                integer(token)?
            }
        };
        loop {
            self.space();
            let rest = &self.source[self.pos..];
            let Some((op, precedence)) = [
                ("||", 1),
                ("&&", 2),
                ("|", 3),
                ("^", 4),
                ("&", 5),
                ("==", 6),
                ("!=", 6),
                ("<=", 7),
                (">=", 7),
                ("<<", 8),
                (">>", 8),
                ("<", 7),
                (">", 7),
                ("+", 9),
                ("-", 9),
                ("*", 10),
                ("/", 10),
                ("%", 10),
            ]
            .into_iter()
            .find(|(op, _)| rest.starts_with(op)) else {
                break;
            };
            if precedence < minimum {
                break;
            }
            self.pos += op.len();
            let right = self.expression(
                precedence + 1,
                active && !(op == "&&" && left == 0 || op == "||" && left != 0),
            )?;
            if !active {
                left = 0;
                continue;
            }
            left = match op {
                "+" => left.wrapping_add(right),
                "-" => left.wrapping_sub(right),
                "*" => left.wrapping_mul(right),
                "/" => left
                    .checked_div(right)
                    .ok_or("division by zero or overflow")?,
                "%" => left
                    .checked_rem(right)
                    .ok_or("division by zero or overflow")?,
                "<<" => left.wrapping_shl(right as u32),
                ">>" => left.wrapping_shr(right as u32),
                "==" => i64::from(left == right),
                "!=" => i64::from(left != right),
                "<" => i64::from(left < right),
                ">" => i64::from(left > right),
                "<=" => i64::from(left <= right),
                ">=" => i64::from(left >= right),
                "&&" => i64::from(left != 0 && right != 0),
                "||" => i64::from(left != 0 || right != 0),
                "&" => left & right,
                "|" => left | right,
                "^" => left ^ right,
                _ => unreachable!(),
            };
        }
        self.depth -= 1;
        Ok(left)
    }
}
fn integer(text: &str) -> Result<i64, String> {
    let text = text.trim();
    let value = if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16)
    } else if text.starts_with('0') && text.len() > 1 {
        i64::from_str_radix(&text[1..], 8)
    } else {
        text.parse()
    };
    value.map_err(|_| format!("invalid arithmetic integer: {text}"))
}
