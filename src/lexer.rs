use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::error::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Kw {
    Var,
    This,
    True,
    False,
    Undefined,
    Null,
    If,
    Else,
    In,
    Function,
    Return,
    For,
    Break,
    Continue,
    Switch,
    Case,
    Default,
    Try,
    Catch,
    Finally,
    Throw,
    New,
    Instanceof,
    Typeof,
    Void,
    While,
    Do,
    Class,
    Super,
    In,
    Delete,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Ident,
    Number,
    Str,
    Kw(Kw),
    Punct,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub text: Rc<str>,
    pub line: usize,
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
}

impl Lexer {
    pub fn new(src: &str) -> Self {
        Lexer {
            chars: src.chars().collect(),
            pos: 0,
            line: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn peek3(&self) -> Option<char> {
        self.chars.get(self.pos + 2).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if let Some(ch) = c {
            if ch == '\n' {
                self.line += 1;
            }
        }
        self.pos += 1;
        c
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, Error> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia();
            if self.pos >= self.chars.len() {
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    text: Rc::from(""),
                    line: self.line,
                });
                break;
            }
            let start_line = self.line;
            let tok = self.next_token()?;
            tokens.push(Token {
                kind: tok.kind,
                text: tok.text,
                line: start_line,
            });
        }
        Ok(tokens)
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.advance();
                }
                Some('/') if self.peek2() == Some('/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                Some('/') if self.peek2() == Some('*') => {
                    self.advance();
                    self.advance();
                    while let Some(c) = self.peek() {
                        if c == '*' && self.peek2() == Some('/') {
                            self.advance();
                            self.advance();
                            break;
                        }
                        self.advance();
                    }
                }
                _ => break,
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, Error> {
        let c = self.peek().unwrap();
        if c.is_ascii_digit() || (c == '.' && self.peek2().map(|x| x.is_ascii_digit()).unwrap_or(false)) {
            return self.number();
        }
        if c == '"' || c == '\'' {
            return self.string(c);
        }
        if c.is_alphabetic() || c == '_' || c == '$' {
            return self.ident();
        }
        self.punct()
    }

    fn number(&mut self) -> Result<Token, Error> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
                self.advance();
            } else {
                break;
            }
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        Ok(Token {
            kind: TokenKind::Number,
            text: Rc::from(s.as_str()),
            line: self.line,
        })
    }

    fn string(&mut self, quote: char) -> Result<Token, Error> {
        self.advance();
        let mut s = String::new();
        loop {
            match self.peek() {
                None => break,
                Some(c) if c == quote => {
                    self.advance();
                    break;
                }
                Some('\\') => {
                    self.advance();
                    let e = self.peek().unwrap_or('\0');
                    self.advance();
                    match e {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        '\\' => s.push('\\'),
                        '\'' => s.push('\''),
                        '"' => s.push('"'),
                        '0' => s.push('\0'),
                        '/' => s.push('/'),
                        other => s.push(other),
                    }
                }
                Some(c) => {
                    s.push(c);
                    self.advance();
                }
            }
        }
        Ok(Token {
            kind: TokenKind::Str,
            text: Rc::from(s.as_str()),
            line: self.line,
        })
    }

    fn ident(&mut self) -> Result<Token, Error> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '$' {
                self.advance();
            } else {
                break;
            }
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        let kind = match s.as_str() {
            "var" => TokenKind::Kw(Kw::Var),
            "this" => TokenKind::Kw(Kw::This),
            "true" => TokenKind::Kw(Kw::True),
            "false" => TokenKind::Kw(Kw::False),
            "undefined" => TokenKind::Kw(Kw::Undefined),
            "null" => TokenKind::Kw(Kw::Null),
            "if" => TokenKind::Kw(Kw::If),
            "else" => TokenKind::Kw(Kw::Else),
            "in" => TokenKind::Kw(Kw::In),
            "function" => TokenKind::Kw(Kw::Function),
            "return" => TokenKind::Kw(Kw::Return),
            "for" => TokenKind::Kw(Kw::For),
            "break" => TokenKind::Kw(Kw::Break),
            "continue" => TokenKind::Kw(Kw::Continue),
            "switch" => TokenKind::Kw(Kw::Switch),
            "case" => TokenKind::Kw(Kw::Case),
            "default" => TokenKind::Kw(Kw::Default),
            "try" => TokenKind::Kw(Kw::Try),
            "catch" => TokenKind::Kw(Kw::Catch),
            "finally" => TokenKind::Kw(Kw::Finally),
            "throw" => TokenKind::Kw(Kw::Throw),
            "new" => TokenKind::Kw(Kw::New),
            "instanceof" => TokenKind::Kw(Kw::Instanceof),
            "typeof" => TokenKind::Kw(Kw::Typeof),
            "void" => TokenKind::Kw(Kw::Void),
            "while" => TokenKind::Kw(Kw::While),
            "do" => TokenKind::Kw(Kw::Do),
            "class" => TokenKind::Kw(Kw::Class),
            "super" => TokenKind::Kw(Kw::Super),
            "in" => TokenKind::Kw(Kw::In),
            "delete" => TokenKind::Kw(Kw::Delete),
            _ => TokenKind::Ident,
        };
        Ok(Token {
            kind,
            text: Rc::from(s.as_str()),
            line: self.line,
        })
    }

    fn punct(&mut self) -> Result<Token, Error> {
        // Three-character operators first.
        if let (Some(a), Some(b), Some(c)) = (self.peek(), self.peek2(), self.peek3()) {
            let three = format!("{a}{b}{c}");
            if three == "===" || three == "!==" {
                self.advance();
                self.advance();
                self.advance();
                return Ok(Token {
                    kind: TokenKind::Punct,
                    text: Rc::from(three),
                    line: self.line,
                });
            }
        }
        // Two-character operators.
        let two = match (self.peek(), self.peek2()) {
            (Some(a), Some(b)) => Some(format!("{a}{b}")),
            _ => None,
        };
        const MULTI: &[&str] = &[
            "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=", "%=",
        ];
        if let Some(t) = two {
            if MULTI.contains(&t.as_str()) {
                self.advance();
                self.advance();
                return Ok(Token {
                    kind: TokenKind::Punct,
                    text: Rc::from(t),
                    line: self.line,
                });
            }
        }
        let c = self.advance().unwrap();
        let text: String = match c {
            '.' | '(' | ')' | '{' | '}' | '[' | ']' | ',' | ';' | ':' | '?' | '=' | '+' | '-'
            | '*' | '/' | '%' | '!' | '<' | '>' => String::from(c),
            _ => return Err(Error::Parse(format!("unexpected character '{c}'"))),
        };
        Ok(Token {
            kind: TokenKind::Punct,
            text: Rc::from(text.as_str()),
            line: self.line,
        })
    }
}
