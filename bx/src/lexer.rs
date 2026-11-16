#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Assign,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Bang,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    PlusPlus,
    MinusMinus,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    Dot,
    Newline,
    Eof,
}

pub type Tokens = Vec<(Tok, u32)>;

pub fn lex(src: &str) -> Result<Tokens, String> {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut out: Tokens = Vec::new();

    macro_rules! push {
        ($t:expr) => {
            out.push(($t, line))
        };
    }

    while i < n {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' => {
                i += 1;
            }
            '\n' => {
                if !matches!(out.last(), Some((Tok::Newline, _))) {
                    push!(Tok::Newline);
                }
                line += 1;
                i += 1;
            }
            '/' => {
                if i + 1 < n && chars[i + 1] == '/' {
                    while i < n && chars[i] != '\n' {
                        i += 1;
                    }
                } else if i + 1 < n && chars[i + 1] == '*' {
                    let start_line = line;
                    i += 2;
                    let mut closed = false;
                    while i + 1 < n {
                        if chars[i] == '*' && chars[i + 1] == '/' {
                            i += 2;
                            closed = true;
                            break;
                        }
                        if chars[i] == '\n' {
                            line += 1;
                        }
                        i += 1;
                    }
                    if !closed {
                        return Err(format!("line {}: unterminated block comment", start_line));
                    }
                } else if i + 1 < n && chars[i + 1] == '=' {
                    push!(Tok::SlashEq);
                    i += 2;
                } else {
                    push!(Tok::Slash);
                    i += 1;
                }
            }
            '"' | '\'' => {
                let quote = c;
                let start_line = line;
                i += 1;
                let mut s = String::new();
                loop {
                    if i >= n {
                        return Err(format!("line {}: unterminated string", start_line));
                    }
                    let ch = chars[i];
                    i += 1;
                    if ch == quote {
                        break;
                    }
                    if ch == '\n' {
                        return Err(format!("line {}: newline inside string", line));
                    }
                    if ch == '\\' {
                        if i >= n {
                            return Err(format!("line {}: unterminated escape", line));
                        }
                        let e = chars[i];
                        i += 1;
                        match e {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            'r' => s.push('\r'),
                            '0' => s.push('\0'),
                            'e' => s.push('\u{1b}'),
                            '\\' => s.push('\\'),
                            '"' => s.push('"'),
                            '\'' => s.push('\''),
                            'u' | 'x' => {
                                let want = if e == 'u' { 4 } else { 2 };
                                if i + want > n {
                                    return Err(format!("line {}: bad \\{} escape", line, e));
                                }
                                let hex: String = chars[i..i + want].iter().collect();
                                i += want;
                                let code = u32::from_str_radix(&hex, 16).map_err(|_| {
                                    format!("line {}: bad \\{} escape '{}'", line, e, hex)
                                })?;
                                match char::from_u32(code) {
                                    Some(ch) => s.push(ch),
                                    None => {
                                        return Err(format!(
                                            "line {}: invalid unicode escape",
                                            line
                                        ))
                                    }
                                }
                            }
                            other => s.push(other),
                        }
                    } else {
                        s.push(ch);
                    }
                }
                push!(Tok::Str(s));
            }
            c if c.is_ascii_digit() => {
                let start = i;
                if c == '0' && i + 1 < n && (chars[i + 1] == 'x' || chars[i + 1] == 'X') {
                    i += 2;
                    let hs = i;
                    while i < n && chars[i].is_ascii_hexdigit() {
                        i += 1;
                    }
                    let text: String = chars[hs..i].iter().collect();
                    let v = i64::from_str_radix(&text, 16)
                        .map_err(|_| format!("line {}: bad hex literal", line))?;
                    push!(Tok::Int(v));
                } else {
                    while i < n && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                    let mut is_float = false;
                    if i + 1 < n && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                        is_float = true;
                        i += 1;
                        while i < n && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    if i < n && (chars[i] == 'e' || chars[i] == 'E') {
                        let save = i;
                        let mut j = i + 1;
                        if j < n && (chars[j] == '+' || chars[j] == '-') {
                            j += 1;
                        }
                        if j < n && chars[j].is_ascii_digit() {
                            is_float = true;
                            i = j;
                            while i < n && chars[i].is_ascii_digit() {
                                i += 1;
                            }
                        } else {
                            i = save;
                        }
                    }
                    let text: String = chars[start..i].iter().collect();
                    if is_float {
                        let v: f64 = text
                            .parse()
                            .map_err(|_| format!("line {}: bad number literal", line))?;
                        push!(Tok::Float(v));
                    } else {
                        let v: i64 = text
                            .parse()
                            .map_err(|_| format!("line {}: number out of range", line))?;
                        push!(Tok::Int(v));
                    }
                }
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                push!(Tok::Ident(text));
            }
            '=' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    push!(Tok::Eq);
                    i += 2;
                } else {
                    push!(Tok::Assign);
                    i += 1;
                }
            }
            '!' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    push!(Tok::Ne);
                    i += 2;
                } else {
                    push!(Tok::Bang);
                    i += 1;
                }
            }
            '<' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    push!(Tok::Le);
                    i += 2;
                } else {
                    push!(Tok::Lt);
                    i += 1;
                }
            }
            '>' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    push!(Tok::Ge);
                    i += 2;
                } else {
                    push!(Tok::Gt);
                    i += 1;
                }
            }
            '&' => {
                if i + 1 < n && chars[i + 1] == '&' {
                    push!(Tok::AndAnd);
                    i += 2;
                } else {
                    return Err(format!("line {}: unexpected '&' (use '&&')", line));
                }
            }
            '|' => {
                if i + 1 < n && chars[i + 1] == '|' {
                    push!(Tok::OrOr);
                    i += 2;
                } else {
                    return Err(format!("line {}: unexpected '|' (use '||')", line));
                }
            }
            '+' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    push!(Tok::PlusEq);
                    i += 2;
                } else if i + 1 < n && chars[i + 1] == '+' {
                    push!(Tok::PlusPlus);
                    i += 2;
                } else {
                    push!(Tok::Plus);
                    i += 1;
                }
            }
            '-' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    push!(Tok::MinusEq);
                    i += 2;
                } else if i + 1 < n && chars[i + 1] == '-' {
                    push!(Tok::MinusMinus);
                    i += 2;
                } else {
                    push!(Tok::Minus);
                    i += 1;
                }
            }
            '*' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    push!(Tok::StarEq);
                    i += 2;
                } else {
                    push!(Tok::Star);
                    i += 1;
                }
            }
            '%' => {
                if i + 1 < n && chars[i + 1] == '=' {
                    push!(Tok::PercentEq);
                    i += 2;
                } else {
                    push!(Tok::Percent);
                    i += 1;
                }
            }
            '(' => {
                push!(Tok::LParen);
                i += 1;
            }
            ')' => {
                push!(Tok::RParen);
                i += 1;
            }
            '{' => {
                push!(Tok::LBrace);
                i += 1;
            }
            '}' => {
                push!(Tok::RBrace);
                i += 1;
            }
            '[' => {
                push!(Tok::LBracket);
                i += 1;
            }
            ']' => {
                push!(Tok::RBracket);
                i += 1;
            }
            ',' => {
                push!(Tok::Comma);
                i += 1;
            }
            ';' => {
                push!(Tok::Semi);
                i += 1;
            }
            ':' => {
                push!(Tok::Colon);
                i += 1;
            }
            '.' => {
                push!(Tok::Dot);
                i += 1;
            }
            other => {
                return Err(format!("line {}: unexpected character '{}'", line, other));
            }
        }
    }
    out.push((Tok::Eof, line));
    Ok(out)
}
