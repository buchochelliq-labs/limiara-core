//! Lexer and parser for the formula language (ADR 0021). Positions are
//! 1-based character offsets, so errors can point at the token.

use super::limits;

#[derive(Debug, Clone, PartialEq)]
pub struct FormulaError {
    pub position: usize,
    pub message: String,
}

impl FormulaError {
    pub fn new(position: usize, message: impl Into<String>) -> Self {
        Self {
            position,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::Concat => "&",
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::And => "&&",
            Self::Or => "||",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Negate,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Number(f64),
    Text(String),
    Bool(bool),
    Null,
    /// A dotted path of API names.
    Path(Vec<String>),
    /// An upper-cased function name and its arguments.
    Call(String, Vec<Expr>),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub position: usize,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Text(String),
    Word(String),
    Colon,
    Dot,
    Comma,
    Open,
    Close,
    Op(&'static str),
}

fn lex(text: &str) -> Result<Vec<(Token, usize)>, FormulaError> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let position = at + 1;
        let current = chars[at];
        if current.is_whitespace() {
            at += 1;
            continue;
        }
        let two: String = chars[at..chars.len().min(at + 2)].iter().collect();
        let token = if let Some(op) = ["==", "!=", "<>", "<=", ">=", "&&", "||"]
            .into_iter()
            .find(|op| *op == two)
        {
            at += 2;
            Token::Op(op)
        } else {
            match current {
                ':' => {
                    at += 1;
                    Token::Colon
                }
                '.' if !chars.get(at + 1).is_some_and(char::is_ascii_digit) => {
                    at += 1;
                    Token::Dot
                }
                ',' => {
                    at += 1;
                    Token::Comma
                }
                '(' => {
                    at += 1;
                    Token::Open
                }
                ')' => {
                    at += 1;
                    Token::Close
                }
                '+' | '-' | '*' | '/' | '&' | '=' | '<' | '>' | '!' => {
                    at += 1;
                    Token::Op(match current {
                        '+' => "+",
                        '-' => "-",
                        '*' => "*",
                        '/' => "/",
                        '&' => "&",
                        '=' => "=",
                        '<' => "<",
                        '>' => ">",
                        _ => "!",
                    })
                }
                '\'' | '"' => {
                    let quote = current;
                    at += 1;
                    let mut value = String::new();
                    loop {
                        let Some(&next) = chars.get(at) else {
                            return Err(FormulaError::new(
                                position,
                                format!("text is not closed with {quote}"),
                            ));
                        };
                        at += 1;
                        match next {
                            next if next == quote => break,
                            '\\' => {
                                let Some(&escaped) = chars.get(at) else {
                                    return Err(FormulaError::new(at, "an escape is incomplete"));
                                };
                                at += 1;
                                value.push(match escaped {
                                    'n' => '\n',
                                    't' => '\t',
                                    'r' => '\r',
                                    '\\' | '\'' | '"' => escaped,
                                    other => {
                                        return Err(FormulaError::new(
                                            at - 1,
                                            format!("'\\{other}' is not an escape"),
                                        ))
                                    }
                                });
                            }
                            other => value.push(other),
                        }
                    }
                    Token::Text(value)
                }
                digit if digit.is_ascii_digit() || digit == '.' => {
                    let start = at;
                    while at < chars.len() && (chars[at].is_ascii_digit() || chars[at] == '.') {
                        at += 1;
                    }
                    let raw: String = chars[start..at].iter().collect();
                    let number = raw
                        .parse::<f64>()
                        .ok()
                        .filter(|number| number.is_finite())
                        .ok_or_else(|| {
                            FormulaError::new(position, format!("'{raw}' is not a number"))
                        })?;
                    Token::Number(number)
                }
                letter if letter.is_ascii_alphabetic() || letter == '_' => {
                    let start = at;
                    while at < chars.len()
                        && (chars[at].is_ascii_alphanumeric() || chars[at] == '_')
                    {
                        at += 1;
                    }
                    Token::Word(chars[start..at].iter().collect())
                }
                other => {
                    return Err(FormulaError::new(
                        position,
                        format!("'{other}' is not expected here"),
                    ))
                }
            }
        };
        tokens.push((token, position));
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<(Token, usize)>,
    at: usize,
    end: usize,
    nodes: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(token, _)| token)
    }

    fn position(&self) -> usize {
        self.tokens
            .get(self.at)
            .map_or(self.end, |(_, position)| *position)
    }

    fn found(&self) -> String {
        match self.peek() {
            None => "the end of the formula".into(),
            Some(Token::Number(_)) => "a number".into(),
            Some(Token::Text(_)) => "text".into(),
            Some(Token::Word(word)) => format!("'{word}'"),
            Some(Token::Op(op)) => format!("'{op}'"),
            Some(Token::Colon) => "':'".into(),
            Some(Token::Dot) => "'.'".into(),
            Some(Token::Comma) => "','".into(),
            Some(Token::Open) => "'('".into(),
            Some(Token::Close) => "')'".into(),
        }
    }

    fn error(&self, expected: &str) -> FormulaError {
        FormulaError::new(
            self.position(),
            format!("expected {expected}, found {}", self.found()),
        )
    }

    fn node(&mut self, kind: ExprKind, position: usize) -> Result<Expr, FormulaError> {
        self.nodes += 1;
        if self.nodes > limits::NODES {
            return Err(FormulaError::new(
                position,
                format!(
                    "the formula is too complex; use at most {} parts",
                    limits::NODES
                ),
            ));
        }
        Ok(Expr { kind, position })
    }

    fn op(&self, ops: &[&'static str]) -> Option<&'static str> {
        match self.peek() {
            Some(Token::Op(op)) if ops.contains(op) => Some(op),
            _ => None,
        }
    }

    fn word(&self, keyword: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(word)) if word.eq_ignore_ascii_case(keyword))
    }

    fn binary(
        &mut self,
        depth: usize,
        ops: &[&'static str],
        next: fn(&mut Self, usize) -> Result<Expr, FormulaError>,
        map: fn(&str) -> BinaryOp,
    ) -> Result<Expr, FormulaError> {
        let mut left = next(self, depth)?;
        while let Some(op) = self.op(ops) {
            let position = self.position();
            self.at += 1;
            let right = next(self, depth)?;
            left = self.node(
                ExprKind::Binary(map(op), Box::new(left), Box::new(right)),
                position,
            )?;
        }
        Ok(left)
    }

    fn expr(&mut self, depth: usize) -> Result<Expr, FormulaError> {
        if depth > limits::DEPTH {
            return Err(FormulaError::new(
                self.position(),
                format!("the formula nests more than {} levels", limits::DEPTH),
            ));
        }
        self.binary(depth, &["||"], Self::and, |_| BinaryOp::Or)
    }

    fn and(&mut self, depth: usize) -> Result<Expr, FormulaError> {
        self.binary(depth, &["&&"], Self::equality, |_| BinaryOp::And)
    }

    fn equality(&mut self, depth: usize) -> Result<Expr, FormulaError> {
        self.binary(depth, &["=", "==", "!=", "<>"], Self::comparison, |op| {
            if op == "=" || op == "==" {
                BinaryOp::Eq
            } else {
                BinaryOp::Ne
            }
        })
    }

    fn comparison(&mut self, depth: usize) -> Result<Expr, FormulaError> {
        self.binary(
            depth,
            &["<", "<=", ">", ">="],
            Self::concat,
            |op| match op {
                "<" => BinaryOp::Lt,
                "<=" => BinaryOp::Le,
                ">" => BinaryOp::Gt,
                _ => BinaryOp::Ge,
            },
        )
    }

    fn concat(&mut self, depth: usize) -> Result<Expr, FormulaError> {
        self.binary(depth, &["&"], Self::additive, |_| BinaryOp::Concat)
    }

    fn additive(&mut self, depth: usize) -> Result<Expr, FormulaError> {
        self.binary(depth, &["+", "-"], Self::term, |op| {
            if op == "+" {
                BinaryOp::Add
            } else {
                BinaryOp::Subtract
            }
        })
    }

    fn term(&mut self, depth: usize) -> Result<Expr, FormulaError> {
        self.binary(depth, &["*", "/"], Self::unary, |op| {
            if op == "*" {
                BinaryOp::Multiply
            } else {
                BinaryOp::Divide
            }
        })
    }

    fn unary(&mut self, depth: usize) -> Result<Expr, FormulaError> {
        let position = self.position();
        if depth > limits::DEPTH {
            return Err(FormulaError::new(
                position,
                format!("the formula nests more than {} levels", limits::DEPTH),
            ));
        }
        let op = if self.op(&["-"]).is_some() {
            Some(UnaryOp::Negate)
        } else if self.op(&["!"]).is_some() {
            Some(UnaryOp::Not)
        } else {
            None
        };
        match op {
            Some(op) => {
                self.at += 1;
                let operand = self.unary(depth + 1)?;
                self.node(ExprKind::Unary(op, Box::new(operand)), position)
            }
            None => self.primary(depth),
        }
    }

    fn name(&mut self) -> Result<String, FormulaError> {
        let Some(Token::Word(first)) = self.peek().cloned() else {
            return Err(self.error("a field name"));
        };
        self.at += 1;
        if self.peek() == Some(&Token::Colon) {
            self.at += 1;
            let Some(Token::Word(second)) = self.peek().cloned() else {
                return Err(self.error("a name after ':'"));
            };
            self.at += 1;
            return Ok(format!("{first}:{second}"));
        }
        Ok(first)
    }

    fn primary(&mut self, depth: usize) -> Result<Expr, FormulaError> {
        let position = self.position();
        match self.peek().cloned() {
            Some(Token::Number(number)) => {
                self.at += 1;
                self.node(ExprKind::Number(number), position)
            }
            Some(Token::Text(text)) => {
                self.at += 1;
                self.node(ExprKind::Text(text), position)
            }
            Some(Token::Open) => {
                self.at += 1;
                let inner = self.expr(depth + 1)?;
                if self.peek() != Some(&Token::Close) {
                    return Err(self.error("')'"));
                }
                self.at += 1;
                Ok(inner)
            }
            Some(Token::Word(word)) => {
                if self.tokens.get(self.at + 1).map(|(token, _)| token) == Some(&Token::Open) {
                    self.at += 2;
                    let mut arguments = Vec::new();
                    if self.peek() != Some(&Token::Close) {
                        loop {
                            arguments.push(self.expr(depth + 1)?);
                            if self.peek() == Some(&Token::Comma) {
                                self.at += 1;
                                continue;
                            }
                            break;
                        }
                    }
                    if self.peek() != Some(&Token::Close) {
                        return Err(self.error("',' or ')'"));
                    }
                    self.at += 1;
                    return self.node(
                        ExprKind::Call(word.to_ascii_uppercase(), arguments),
                        position,
                    );
                }
                for (keyword, kind) in [
                    ("TRUE", ExprKind::Bool(true)),
                    ("FALSE", ExprKind::Bool(false)),
                    ("NULL", ExprKind::Null),
                ] {
                    if self.word(keyword) {
                        self.at += 1;
                        return self.node(kind, position);
                    }
                }
                let mut segments = vec![self.name()?];
                while self.peek() == Some(&Token::Dot) {
                    self.at += 1;
                    segments.push(self.name()?);
                }
                self.node(ExprKind::Path(segments), position)
            }
            _ => Err(self.error("a value, a field or a function")),
        }
    }
}

/// Parses formula text into a syntax tree.
pub fn parse(text: &str) -> Result<Expr, FormulaError> {
    if text.chars().count() > limits::LENGTH {
        return Err(FormulaError::new(
            limits::LENGTH,
            format!("use at most {} characters", limits::LENGTH),
        ));
    }
    let tokens = lex(text)?;
    let mut parser = Parser {
        tokens,
        at: 0,
        end: text.chars().count() + 1,
        nodes: 0,
    };
    let expr = parser.expr(0)?;
    if parser.peek().is_some() {
        return Err(parser.error("the end of the formula"));
    }
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Number(number) => number.to_string(),
            ExprKind::Text(text) => format!("'{text}'"),
            ExprKind::Bool(flag) => flag.to_string().to_uppercase(),
            ExprKind::Null => "NULL".into(),
            ExprKind::Path(segments) => segments.join("."),
            ExprKind::Call(name, arguments) => format!(
                "{name}({})",
                arguments.iter().map(shape).collect::<Vec<_>>().join(", ")
            ),
            ExprKind::Unary(UnaryOp::Negate, operand) => format!("(-{})", shape(operand)),
            ExprKind::Unary(UnaryOp::Not, operand) => format!("(!{})", shape(operand)),
            ExprKind::Binary(op, left, right) => {
                format!("({} {} {})", shape(left), op.symbol(), shape(right))
            }
        }
    }

    #[test]
    fn precedence_follows_the_usual_rules() {
        for (text, expected) in [
            ("1 + 2 * 3", "(1 + (2 * 3))"),
            ("a = 1 && b <> 2 || !c", "(((a = 1) && (b != 2)) || (!c))"),
            ("'x' & custom:name + 1", "('x' & (custom:name + 1))"),
            (
                "-custom:a.custom:b.name < 3",
                "((-custom:a.custom:b.name) < 3)",
            ),
            ("if(isblank(x), null, .5)", "IF(ISBLANK(x), NULL, 0.5)"),
            ("TODAY() - 1", "(TODAY() - 1)"),
            ("\"it's\"", "'it's'"),
        ] {
            assert_eq!(shape(&parse(text).expect(text)), expected, "{text}");
        }
    }

    #[test]
    fn errors_point_at_the_problem() {
        for (text, position, fragment) in [
            ("1 +", 4, "end of the formula"),
            ("IF(a, b", 8, "',' or ')'"),
            ("'open", 1, "not closed"),
            ("a # b", 3, "'#'"),
            ("a b", 3, "end of the formula"),
            ("custom:", 8, "a name after ':'"),
        ] {
            let error = parse(text).expect_err(text);
            assert_eq!(error.position, position, "{text}: {}", error.message);
            assert!(
                error.message.contains(fragment),
                "{text}: {}",
                error.message
            );
        }
    }

    #[test]
    fn size_and_nesting_are_bounded() {
        assert!(parse(&"x".repeat(limits::LENGTH + 1)).is_err());
        let deep = format!(
            "{}1{}",
            "(".repeat(limits::DEPTH + 2),
            ")".repeat(limits::DEPTH + 2)
        );
        assert!(parse(&deep).unwrap_err().message.contains("nests"));
        let wide = vec!["1"; limits::NODES + 1].join(" + ");
        assert!(parse(&wide).unwrap_err().message.contains("too complex"));
        let negations = format!("{}1", "-".repeat(limits::DEPTH + 2));
        assert!(parse(&negations).is_err());
    }
}
