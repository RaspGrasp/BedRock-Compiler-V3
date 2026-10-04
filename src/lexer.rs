#[derive(Debug, Clone, PartialEq)]
pub enum Token {

    Fn, Let, Loop, While, Asm, If, Else, Return, Root, Inb, Outb, Break, Poke, Peek, Include, Call, Struct, Dot, Bnw, Int, IntEnable, IntDisable, SaveCtx, RestoreCtx, Syscall,

    U8, U16, U32, U64,
    I8, I16, I32, I64,
    Bool,

    Identifier(String), Number(u64), StringLiteral(String),

    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    Colon, SemiColon, Comma, Equal,
    At,  
    ShiftLeft, ShiftRight,
    Plus, Minus, Star, Slash,
    EqEq, NotEq, Greater, Less, GreaterEq, LessEq,
    Ampersand, Pipe, Caret, Percent, Bang,
    AndAnd, OrOr,
    EOF,
}

pub struct Lexer {
    input: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl Lexer {
    pub fn line(&self) -> usize { self.line }
    pub fn col(&self) -> usize { self.col }

    pub fn new(input: &str) -> Self {
        Lexer {
            input: input.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_whitespace_and_comments();
        if self.pos >= self.input.len() { return Token::EOF; }

        let ch = self.input[self.pos];

        if ch.is_alphabetic() || ch == '_' { return self.read_identifier(); }
        if ch.is_digit(10) { return self.read_number(); }

        match ch {
            '(' => { self.advance_char(); Token::LParen }
            ')' => { self.advance_char(); Token::RParen }
            '{' => { self.advance_char(); Token::LBrace }
            '}' => { self.advance_char(); Token::RBrace }
            '[' => { self.advance_char(); Token::LBracket }
            ']' => { self.advance_char(); Token::RBracket }
            ';' => { self.advance_char(); Token::SemiColon }
            ',' => { self.advance_char(); Token::Comma }
            '+' => { self.advance_char(); Token::Plus }
            '-' => { self.advance_char(); Token::Minus }
            '^' => { self.advance_char(); Token::Caret }
            '*' => { self.advance_char(); Token::Star }
            '%' => { self.advance_char(); Token::Percent }
            '&' => {
                self.advance_char();
                if self.pos < self.input.len() && self.input[self.pos] == '&' {
                    self.advance_char();
                    Token::AndAnd
                } else {
                    Token::Ampersand
                }
            }
            '|' => {
                self.advance_char();
                if self.pos < self.input.len() && self.input[self.pos] == '|' {
                    self.advance_char();
                    Token::OrOr
                } else {
                    Token::Pipe
                }
            }
            '@' => { self.advance_char(); Token::At }
            '.' => { self.advance_char(); Token::Dot }
            '/' => {
                self.advance_char();
                if self.pos < self.input.len() && self.input[self.pos] == '/' {
                    self.skip_line_comment();
                    return self.next_token();
                } else if self.pos < self.input.len() && self.input[self.pos] == '*' {
                    self.skip_block_comment();
                    return self.next_token();
                }
                Token::Slash
            }
            '=' => {
                self.advance_char();
                if self.pos < self.input.len() && self.input[self.pos] == '=' {
                    self.advance_char(); Token::EqEq
                } else { Token::Equal }
            }
            '!' => {
                self.advance_char();
                if self.pos < self.input.len() && self.input[self.pos] == '=' {
                    self.advance_char(); Token::NotEq
                } else {
                    Token::Bang
                }
            }
            '>' => {
                self.advance_char();
                if self.pos < self.input.len() && self.input[self.pos] == '=' {
                    self.advance_char(); Token::GreaterEq
                } else if self.pos < self.input.len() && self.input[self.pos] == '>' {
                    self.advance_char(); Token::ShiftRight
                } else { Token::Greater }
            }
            '<' => {
                self.advance_char();
                if self.pos < self.input.len() && self.input[self.pos] == '=' {
                    self.advance_char(); Token::LessEq
                } else if self.pos < self.input.len() && self.input[self.pos] == '<' {
                    self.advance_char(); Token::ShiftLeft
                } else { Token::Less }
            }
            '"' => {
                self.advance_char();
                self.read_string()
            }
            _ => {
                if ch.is_whitespace() {
                    self.advance_char();
                    return self.next_token();
                }
                eprintln!("[LEXER ERROR] Unknown character '{}' at line {}, col {}", ch, self.line, self.col);
                self.advance_char();
                self.next_token()
            }
        }
    }

    fn advance_char(&mut self) {
        if self.pos < self.input.len() {
            if self.input[self.pos] == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            self.pos += 1;
        }
    }

    fn skip_whitespace_and_comments(&mut self) {
        while self.pos < self.input.len() && self.input[self.pos].is_whitespace() {
            self.advance_char();
        }
    }

    fn skip_line_comment(&mut self) {
        while self.pos < self.input.len() && self.input[self.pos] != '\n' {
            self.advance_char();
        }
    }

    fn skip_block_comment(&mut self) {
        self.advance_char();
        while self.pos + 1 < self.input.len() {
            if self.input[self.pos] == '*' && self.input[self.pos + 1] == '/' {
                self.advance_char();
                self.advance_char();
                return;
            }
            self.advance_char();
        }
        eprintln!("[LEXER ERROR] Unterminated block comment");
    }

    fn read_identifier(&mut self) -> Token {
        let start = self.pos;
        while self.pos < self.input.len() &&
              (self.input[self.pos].is_alphanumeric() || self.input[self.pos] == '_') {
            self.advance_char();
        }
        let s: String = self.input[start..self.pos].iter().collect();
        match s.as_str() {

            "fn"      => Token::Fn,
            "let"     => Token::Let,
            "loop"    => Token::Loop,
            "while"   => Token::While,
            "asm"     => Token::Asm,
            "syscall" => Token::Syscall,
            "if"      => Token::If,
            "else"    => Token::Else,
            "return"  => Token::Return,
            "root"    => Token::Root,
            "inb"     => Token::Inb,
            "outb"    => Token::Outb,
            "break"   => Token::Break,
            "poke"    => Token::Poke,
            "peek"    => Token::Peek,
            "include" => Token::Include,
            "call"    => Token::Call,
            "struct" => Token::Struct,

            "int"         => Token::Int,
            "int_enable"  => Token::IntEnable,
            "int_disable" => Token::IntDisable,

            "save_ctx"    => Token::SaveCtx,
            "restore_ctx" => Token::RestoreCtx,

            "u8"      => Token::U8,
            "u16"     => Token::U16,
            "u32"     => Token::U32,
            "u64"     => Token::U64,
            "i8"      => Token::I8,
            "i16"     => Token::I16,
            "i32"     => Token::I32,
            "i64"     => Token::I64,
            "bool"    => Token::Bool,
            _ if s.to_lowercase() == "bnw" => Token::Bnw,
          _ => Token::Identifier(s),
        }
    }

    fn read_number(&mut self) -> Token {
        let mut base = 10u32;
        let mut start = self.pos;

        if self.input[self.pos] == '0' && self.pos + 1 < self.input.len() {
            let next = self.input[self.pos + 1].to_ascii_lowercase();
            if next == 'x' {
                base = 16;
                self.advance_char();
                self.advance_char();
                start = self.pos;
            }
        }

        while self.pos < self.input.len() {
            let ch = self.input[self.pos].to_ascii_lowercase();
            if (base == 10 && !ch.is_ascii_digit()) || (base == 16 && !ch.is_ascii_hexdigit()) {
                break;
            }
            self.advance_char();
        }

        let s: String = self.input[start..self.pos].iter().collect();
        let value = u64::from_str_radix(&s, base).unwrap_or_else(|_| {
            eprintln!("[LEXER ERROR] Invalid number format: {}", s);
            0
        });
        Token::Number(value)
    }

        fn read_string(&mut self) -> Token {
        // pos is at opening quote
        if self.pos < self.input.len() && self.input[self.pos] == '"' {
            self.advance_char();
        }
        let mut s = String::new();
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch == '"' {
                self.advance_char();
                break;
            }
            if ch == '\\' {
                self.advance_char();
                if self.pos >= self.input.len() { break; }
                let e = self.input[self.pos];
                let mapped = match e {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    '0' => '\0',
                    '\\' => '\\',
                    '"' => '"',
                    other => other,
                };
                s.push(mapped);
                self.advance_char();
            } else {
                s.push(ch);
                self.advance_char();
            }
        }
        Token::StringLiteral(s)
    }
}