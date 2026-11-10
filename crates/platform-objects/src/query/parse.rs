//! Lexer and parser for the record query language (ADR 0019).
//!
//! The parser knows nothing about metadata: it turns text into a syntax tree
//! whose names are still strings. Positions are 1-based character offsets
//! into the query text, so errors can point at the offending token.

use chrono::{DateTime, NaiveDate, Utc};

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub position: usize,
    pub message: String,
}

impl ParseError {
    fn new(position: usize, message: impl Into<String>) -> Self {
        Self {
            position,
            message: message.into(),
        }
    }
}

/// A dotted path of API names: `custom:account.name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub segments: Vec<String>,
    pub position: usize,
}

impl Path {
    pub fn text(&self) -> String {
        self.segments.join(".")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregate {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl Aggregate {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Sum => "sum",
            Self::Avg => "avg",
            Self::Min => "min",
            Self::Max => "max",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        match word.to_ascii_uppercase().as_str() {
            "COUNT" => Some(Self::Count),
            "SUM" => Some(Self::Sum),
            "AVG" => Some(Self::Avg),
            "MIN" => Some(Self::Min),
            "MAX" => Some(Self::Max),
            _ => None,
        }
    }
}

/// An aggregate call: `COUNT()`, `SUM(custom:amount)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateCall {
    pub function: Aggregate,
    pub argument: Option<Path>,
    pub position: usize,
}

impl AggregateCall {
    /// The key a result row uses when the call has no alias.
    pub fn text(&self) -> String {
        format!(
            "{}({})",
            self.function.as_str(),
            self.argument.as_ref().map(Path::text).unwrap_or_default()
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    Path {
        path: Path,
        alias: Option<String>,
    },
    Aggregate {
        call: AggregateCall,
        alias: Option<String>,
    },
}

/// A period relative to today (ADR 0019 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativeDate {
    Today,
    Yesterday,
    Tomorrow,
    ThisWeek,
    LastWeek,
    NextWeek,
    ThisMonth,
    LastMonth,
    NextMonth,
    ThisYear,
    LastNDays(u32),
    NextNDays(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// `value` is the text with escapes resolved; `pattern` is the same text
    /// as a LIKE pattern with `\` as the escape character, where only the
    /// unescaped `%` and `_` are wildcards.
    Text {
        value: String,
        pattern: String,
    },
    Number(f64),
    Bool(bool),
    Null,
    Date(NaiveDate),
    DateTime(DateTime<Utc>),
    Relative(RelativeDate),
}

impl Literal {
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Number(_) => "a number",
            Self::Bool(_) => "true or false",
            Self::Null => "null",
            Self::Date(_) | Self::Relative(_) => "a date",
            Self::DateTime(_) => "a date-time",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operator {
    Compare(Comparison, Literal),
    In { negated: bool, values: Vec<Literal> },
    Like { negated: bool, pattern: Literal },
    Includes { negated: bool, values: Vec<Literal> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub path: Path,
    pub operator: Operator,
    pub position: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Condition(Condition),
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrderTarget {
    /// A path, or an alias when it has one segment that names one.
    Path(Path),
    Aggregate(AggregateCall),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderItem {
    pub target: OrderTarget,
    pub descending: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub select: Vec<SelectItem>,
    pub from: Path,
    pub filter: Option<Expr>,
    pub group_by: Vec<Path>,
    pub order_by: Vec<OrderItem>,
    pub limit: Option<(u64, usize)>,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Text { value: String, pattern: String },
    Number(f64),
    Date(NaiveDate),
    DateTime(DateTime<Utc>),
    Colon,
    Dot,
    Comma,
    Open,
    Close,
    Op(Comparison),
}

impl Token {
    fn describe(&self) -> String {
        match self {
            Self::Word(word) => format!("'{word}'"),
            Self::Text { .. } => "text".into(),
            Self::Number(_) => "a number".into(),
            Self::Date(_) | Self::DateTime(_) => "a date".into(),
            Self::Colon => "':'".into(),
            Self::Dot => "'.'".into(),
            Self::Comma => "','".into(),
            Self::Open => "'('".into(),
            Self::Close => "')'".into(),
            Self::Op(_) => "an operator".into(),
        }
    }
}

fn lex(text: &str) -> Result<Vec<(Token, usize)>, ParseError> {
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
        let token = match current {
            ':' => {
                at += 1;
                Token::Colon
            }
            '.' => {
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
            '=' => {
                at += 1;
                Token::Op(Comparison::Eq)
            }
            '!' if chars.get(at + 1) == Some(&'=') => {
                at += 2;
                Token::Op(Comparison::Ne)
            }
            '<' => match chars.get(at + 1) {
                Some('=') => {
                    at += 2;
                    Token::Op(Comparison::Le)
                }
                Some('>') => {
                    at += 2;
                    Token::Op(Comparison::Ne)
                }
                _ => {
                    at += 1;
                    Token::Op(Comparison::Lt)
                }
            },
            '>' => {
                if chars.get(at + 1) == Some(&'=') {
                    at += 2;
                    Token::Op(Comparison::Ge)
                } else {
                    at += 1;
                    Token::Op(Comparison::Gt)
                }
            }
            '\'' => {
                at += 1;
                let mut value = String::new();
                let mut pattern = String::new();
                loop {
                    let Some(&next) = chars.get(at) else {
                        return Err(ParseError::new(position, "text is not closed with '"));
                    };
                    at += 1;
                    match next {
                        '\'' => break,
                        '\\' => {
                            let Some(&escaped) = chars.get(at) else {
                                return Err(ParseError::new(at, "an escape is incomplete"));
                            };
                            at += 1;
                            let (resolved, like) = match escaped {
                                '\'' => ('\'', "'"),
                                '\\' => ('\\', "\\\\"),
                                'n' => ('\n', "\n"),
                                't' => ('\t', "\t"),
                                'r' => ('\r', "\r"),
                                '%' => ('%', "\\%"),
                                '_' => ('_', "\\_"),
                                other => {
                                    return Err(ParseError::new(
                                        at - 1,
                                        format!("'\\{other}' is not an escape"),
                                    ))
                                }
                            };
                            value.push(resolved);
                            pattern.push_str(like);
                        }
                        other => {
                            value.push(other);
                            pattern.push(other);
                        }
                    }
                }
                Token::Text { value, pattern }
            }
            digit if digit.is_ascii_digit() || (digit == '-' && next_is_digit(&chars, at)) => {
                let start = at;
                at += 1;
                while at < chars.len() && (chars[at].is_ascii_digit() || chars[at] == '.') {
                    at += 1;
                }
                // Four digits and a dash start a date or date-time.
                if at - start == 4 && current != '-' && chars.get(at) == Some(&'-') {
                    while at < chars.len()
                        && (chars[at].is_ascii_alphanumeric()
                            || matches!(chars[at], '-' | ':' | '.' | '+'))
                    {
                        at += 1;
                    }
                }
                let raw: String = chars[start..at].iter().collect();
                literal_number_or_date(&raw, position)?
            }
            letter if letter.is_ascii_alphabetic() || letter == '_' => {
                let start = at;
                while at < chars.len() && (chars[at].is_ascii_alphanumeric() || chars[at] == '_') {
                    at += 1;
                }
                // A plugin's dotted namespace is one word (ADR 0015 §3).
                let after_colon = matches!(tokens.last(), Some((Token::Colon, _)));
                at = crate::naming::namespace_word_end(&chars, start, at, after_colon);
                Token::Word(chars[start..at].iter().collect())
            }
            other => {
                return Err(ParseError::new(
                    position,
                    format!("'{other}' is not expected here"),
                ))
            }
        };
        tokens.push((token, position));
    }
    Ok(tokens)
}

fn next_is_digit(chars: &[char], at: usize) -> bool {
    chars.get(at + 1).is_some_and(char::is_ascii_digit)
}

fn looks_like_date(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() > 4 && bytes[..4].iter().all(u8::is_ascii_digit) && bytes[4] == b'-'
}

fn literal_number_or_date(raw: &str, position: usize) -> Result<Token, ParseError> {
    if looks_like_date(raw) {
        if raw.len() == 10 {
            return NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .map(Token::Date)
                .map_err(|_| ParseError::new(position, format!("'{raw}' is not a date")));
        }
        return DateTime::parse_from_rfc3339(raw)
            .map(|time| Token::DateTime(time.with_timezone(&Utc)))
            .map_err(|_| {
                ParseError::new(
                    position,
                    format!("'{raw}' is not a date-time; use 2026-10-11T09:30:00Z"),
                )
            });
    }
    raw.parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .map(Token::Number)
        .ok_or_else(|| ParseError::new(position, format!("'{raw}' is not a number")))
}

fn relative_date(word: &str) -> Option<RelativeDate> {
    Some(match word.to_ascii_uppercase().as_str() {
        "TODAY" => RelativeDate::Today,
        "YESTERDAY" => RelativeDate::Yesterday,
        "TOMORROW" => RelativeDate::Tomorrow,
        "THIS_WEEK" => RelativeDate::ThisWeek,
        "LAST_WEEK" => RelativeDate::LastWeek,
        "NEXT_WEEK" => RelativeDate::NextWeek,
        "THIS_MONTH" => RelativeDate::ThisMonth,
        "LAST_MONTH" => RelativeDate::LastMonth,
        "NEXT_MONTH" => RelativeDate::NextMonth,
        "THIS_YEAR" => RelativeDate::ThisYear,
        _ => return None,
    })
}

/// Words that cannot be field names or aliases without a namespace.
const RESERVED: [&str; 21] = [
    "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "IN", "LIKE", "INCLUDES", "EXCLUDES", "GROUP",
    "ORDER", "BY", "ASC", "DESC", "LIMIT", "NULL", "TRUE", "FALSE", "AS", "NULLS",
];

fn reserved(word: &str) -> bool {
    RESERVED
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(word))
}

struct Parser {
    tokens: Vec<(Token, usize)>,
    at: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(token, _)| token)
    }

    fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.at + offset).map(|(token, _)| token)
    }

    fn position(&self) -> usize {
        self.tokens
            .get(self.at)
            .map_or(self.end, |(_, position)| *position)
    }

    fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).map(|(token, _)| token.clone());
        self.at += 1;
        token
    }

    fn error(&self, expected: &str) -> ParseError {
        let found = self
            .peek()
            .map_or_else(|| "the end of the query".to_owned(), Token::describe);
        ParseError::new(
            self.position(),
            format!("expected {expected}, found {found}"),
        )
    }

    fn at_keyword(&self, keyword: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(word)) if word.eq_ignore_ascii_case(keyword))
    }

    fn keyword(&mut self, keyword: &str) -> bool {
        if self.at_keyword(keyword) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, keyword: &str) -> Result<(), ParseError> {
        if self.keyword(keyword) {
            Ok(())
        } else {
            Err(self.error(keyword))
        }
    }

    fn expect(&mut self, token: &Token, expected: &str) -> Result<(), ParseError> {
        if self.peek() == Some(token) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.error(expected))
        }
    }

    /// `ident` or `ident:ident`.
    fn name(&mut self) -> Result<String, ParseError> {
        let Some(Token::Word(first)) = self.peek().cloned() else {
            return Err(self.error("a name"));
        };
        if self.peek_at(1) == Some(&Token::Colon) {
            if let Some(Token::Word(second)) = self.peek_at(2).cloned() {
                self.at += 3;
                return Ok(format!("{first}:{second}"));
            }
        }
        if reserved(&first) {
            return Err(self.error("a name"));
        }
        self.at += 1;
        Ok(first)
    }

    fn path(&mut self) -> Result<Path, ParseError> {
        let position = self.position();
        let mut segments = vec![self.name()?];
        while self.peek() == Some(&Token::Dot) {
            self.at += 1;
            segments.push(self.name()?);
        }
        Ok(Path { segments, position })
    }

    fn aggregate_call(&mut self) -> Result<Option<AggregateCall>, ParseError> {
        let Some(Token::Word(word)) = self.peek() else {
            return Ok(None);
        };
        let Some(function) = Aggregate::parse(word) else {
            return Ok(None);
        };
        if self.peek_at(1) != Some(&Token::Open) {
            return Ok(None);
        }
        let position = self.position();
        self.at += 2;
        let argument = if self.peek() == Some(&Token::Close) {
            None
        } else {
            Some(self.path()?)
        };
        self.expect(&Token::Close, "')'")?;
        if argument.is_none() && function != Aggregate::Count {
            return Err(ParseError::new(
                position,
                format!("{} needs a field", function.as_str().to_uppercase()),
            ));
        }
        Ok(Some(AggregateCall {
            function,
            argument,
            position,
        }))
    }

    fn alias(&mut self) -> Result<Option<String>, ParseError> {
        let explicit = self.keyword("AS");
        match self.peek() {
            Some(Token::Word(word))
                if !reserved(word) && self.peek_at(1) != Some(&Token::Colon) =>
            {
                let alias = word.clone();
                self.at += 1;
                Ok(Some(alias))
            }
            _ if explicit => Err(self.error("an alias")),
            _ => Ok(None),
        }
    }

    fn select_item(&mut self) -> Result<SelectItem, ParseError> {
        if let Some(call) = self.aggregate_call()? {
            let alias = self.alias()?;
            return Ok(SelectItem::Aggregate { call, alias });
        }
        if self.peek() == Some(&Token::Open) {
            return Err(ParseError::new(
                self.position(),
                "child subqueries are not supported yet",
            ));
        }
        let path = self.path()?;
        let alias = self.alias()?;
        Ok(SelectItem::Path { path, alias })
    }

    fn literal(&mut self) -> Result<Literal, ParseError> {
        let position = self.position();
        match self.advance() {
            Some(Token::Text { value, pattern }) => Ok(Literal::Text { value, pattern }),
            Some(Token::Number(number)) => Ok(Literal::Number(number)),
            Some(Token::Date(date)) => Ok(Literal::Date(date)),
            Some(Token::DateTime(time)) => Ok(Literal::DateTime(time)),
            Some(Token::Word(word)) => {
                let upper = word.to_ascii_uppercase();
                match upper.as_str() {
                    "TRUE" => return Ok(Literal::Bool(true)),
                    "FALSE" => return Ok(Literal::Bool(false)),
                    "NULL" => return Ok(Literal::Null),
                    "LAST_N_DAYS" | "NEXT_N_DAYS" => {
                        self.expect(&Token::Colon, "':' and a number of days")?;
                        let days_position = self.position();
                        let Some(Token::Number(days)) = self.advance() else {
                            return Err(ParseError::new(
                                days_position,
                                "expected a number of days",
                            ));
                        };
                        if days.fract() != 0.0 || !(0.0..=3650.0).contains(&days) {
                            return Err(ParseError::new(
                                days_position,
                                "use a whole number of days from 0 to 3650",
                            ));
                        }
                        // In range and whole, checked above.
                        let days = days as u32;
                        return Ok(Literal::Relative(if upper == "LAST_N_DAYS" {
                            RelativeDate::LastNDays(days)
                        } else {
                            RelativeDate::NextNDays(days)
                        }));
                    }
                    _ => {}
                }
                relative_date(&word).map(Literal::Relative).ok_or_else(|| {
                    ParseError::new(position, format!("expected a value, found '{word}'"))
                })
            }
            Some(other) => Err(ParseError::new(
                position,
                format!("expected a value, found {}", other.describe()),
            )),
            None => Err(ParseError::new(
                position,
                "expected a value, found the end of the query",
            )),
        }
    }

    fn literal_list(&mut self) -> Result<Vec<Literal>, ParseError> {
        self.expect(&Token::Open, "'('")?;
        let mut values = vec![self.literal()?];
        while self.peek() == Some(&Token::Comma) {
            self.at += 1;
            values.push(self.literal()?);
        }
        self.expect(&Token::Close, "')'")?;
        Ok(values)
    }

    fn condition(&mut self) -> Result<Expr, ParseError> {
        let position = self.position();
        let path = self.path()?;
        let negated = self.keyword("NOT");
        let operator = if self.keyword("IN") {
            Operator::In {
                negated,
                values: self.literal_list()?,
            }
        } else if self.keyword("LIKE") {
            Operator::Like {
                negated,
                pattern: self.literal()?,
            }
        } else if negated {
            return Err(self.error("IN or LIKE"));
        } else if self.keyword("INCLUDES") {
            Operator::Includes {
                negated: false,
                values: self.literal_list()?,
            }
        } else if self.keyword("EXCLUDES") {
            Operator::Includes {
                negated: true,
                values: self.literal_list()?,
            }
        } else if let Some(Token::Op(comparison)) = self.peek().cloned() {
            self.at += 1;
            Operator::Compare(comparison, self.literal()?)
        } else {
            return Err(self.error("an operator"));
        };
        Ok(Expr::Condition(Condition {
            path,
            operator,
            position,
        }))
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        if self.keyword("NOT") {
            return Ok(Expr::Not(Box::new(self.primary()?)));
        }
        if self.peek() == Some(&Token::Open) {
            self.at += 1;
            let expr = self.or()?;
            self.expect(&Token::Close, "')'")?;
            return Ok(expr);
        }
        self.condition()
    }

    fn and(&mut self) -> Result<Expr, ParseError> {
        let mut terms = vec![self.primary()?];
        while self.keyword("AND") {
            terms.push(self.primary()?);
        }
        Ok(if terms.len() == 1 {
            terms.remove(0)
        } else {
            Expr::And(terms)
        })
    }

    fn or(&mut self) -> Result<Expr, ParseError> {
        let mut terms = vec![self.and()?];
        while self.keyword("OR") {
            terms.push(self.and()?);
        }
        Ok(if terms.len() == 1 {
            terms.remove(0)
        } else {
            Expr::Or(terms)
        })
    }

    fn query(&mut self) -> Result<Query, ParseError> {
        self.expect_keyword("SELECT")?;
        let mut select = vec![self.select_item()?];
        while self.peek() == Some(&Token::Comma) {
            self.at += 1;
            select.push(self.select_item()?);
        }
        self.expect_keyword("FROM")?;
        let from = self.path()?;
        let filter = if self.keyword("WHERE") {
            Some(self.or()?)
        } else {
            None
        };
        let mut group_by = Vec::new();
        if self.keyword("GROUP") {
            self.expect_keyword("BY")?;
            group_by.push(self.path()?);
            while self.peek() == Some(&Token::Comma) {
                self.at += 1;
                group_by.push(self.path()?);
            }
        }
        let mut order_by = Vec::new();
        if self.keyword("ORDER") {
            self.expect_keyword("BY")?;
            loop {
                let target = match self.aggregate_call()? {
                    Some(call) => OrderTarget::Aggregate(call),
                    None => OrderTarget::Path(self.path()?),
                };
                let descending = if self.keyword("DESC") {
                    true
                } else {
                    self.keyword("ASC");
                    false
                };
                if self.at_keyword("NULLS") {
                    return Err(ParseError::new(
                        self.position(),
                        "records without a value always sort last; NULLS is not supported",
                    ));
                }
                order_by.push(OrderItem { target, descending });
                if self.peek() != Some(&Token::Comma) {
                    break;
                }
                self.at += 1;
            }
        }
        let limit = if self.keyword("LIMIT") {
            let position = self.position();
            match self.advance() {
                Some(Token::Number(number)) if number.fract() == 0.0 && number >= 1.0 => {
                    // Bounded by the caller's limit check; saturates above u64.
                    Some((number as u64, position))
                }
                _ => {
                    return Err(ParseError::new(
                        position,
                        "LIMIT needs a whole number of at least 1",
                    ))
                }
            }
        } else {
            None
        };
        if self.peek().is_some() {
            return Err(self.error("the end of the query"));
        }
        Ok(Query {
            select,
            from,
            filter,
            group_by,
            order_by,
            limit,
        })
    }
}

/// Parses query text into a syntax tree.
pub fn parse(text: &str) -> Result<Query, ParseError> {
    let tokens = lex(text)?;
    let end = text.chars().count() + 1;
    Parser { tokens, at: 0, end }.query()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Literal {
        Literal::Text {
            value: value.into(),
            pattern: value.into(),
        }
    }

    #[test]
    fn parses_a_full_query() {
        let query = parse(
            "select name, custom:account.name acct, COUNT() FROM custom:deal \
             WHERE custom:stage IN ('won', 'lost') AND NOT (custom:amount >= 10.5 OR owner = null) \
             AND created_at = LAST_N_DAYS:30 AND custom:close != 2026-10-11 \
             GROUP BY name ORDER BY custom:amount DESC, name LIMIT 10",
        )
        .expect("parses");
        assert_eq!(query.from.text(), "custom:deal");
        assert_eq!(query.select.len(), 3);
        assert!(matches!(
            &query.select[1],
            SelectItem::Path { path, alias: Some(alias) } if path.text() == "custom:account.name" && alias == "acct"
        ));
        assert!(matches!(
            &query.select[2],
            SelectItem::Aggregate { call, alias: None } if call.text() == "count()"
        ));
        let Some(Expr::And(terms)) = &query.filter else {
            panic!("and")
        };
        assert_eq!(terms.len(), 4);
        assert!(matches!(
            &terms[0],
            Expr::Condition(Condition { operator: Operator::In { negated: false, values }, .. })
                if values == &vec![text("won"), text("lost")]
        ));
        assert!(matches!(&terms[1], Expr::Not(inner) if matches!(**inner, Expr::Or(_))));
        assert!(matches!(
            &terms[2],
            Expr::Condition(Condition {
                operator: Operator::Compare(
                    Comparison::Eq,
                    Literal::Relative(RelativeDate::LastNDays(30))
                ),
                ..
            })
        ));
        assert!(matches!(
            &terms[3],
            Expr::Condition(Condition {
                operator: Operator::Compare(Comparison::Ne, Literal::Date(_)),
                ..
            })
        ));
        assert_eq!(query.group_by.len(), 1);
        assert_eq!(query.order_by.len(), 2);
        assert!(query.order_by[0].descending);
        assert_eq!(query.limit.map(|(limit, _)| limit), Some(10));
    }

    #[test]
    fn literals_are_typed_and_escaped() {
        let query = parse(
            "SELECT id FROM custom:x WHERE name LIKE 'it\\'s 50\\% _off\\\\' AND custom:n < -3 \
             AND custom:t > 2026-10-11T09:30:00+02:00 AND custom:f = FALSE",
        )
        .expect("parses");
        let Some(Expr::And(terms)) = query.filter else {
            panic!("and")
        };
        let Expr::Condition(Condition {
            operator: Operator::Like { pattern, .. },
            ..
        }) = &terms[0]
        else {
            panic!("like")
        };
        assert_eq!(
            pattern,
            &Literal::Text {
                value: "it's 50% _off\\".into(),
                pattern: "it's 50\\% _off\\\\".into(),
            }
        );
        assert!(matches!(
            &terms[1],
            Expr::Condition(Condition { operator: Operator::Compare(Comparison::Lt, Literal::Number(n)), .. }) if *n == -3.0
        ));
        let Expr::Condition(Condition {
            operator: Operator::Compare(_, Literal::DateTime(time)),
            ..
        }) = &terms[2]
        else {
            panic!("date-time")
        };
        assert_eq!(time.to_rfc3339(), "2026-10-11T07:30:00+00:00");
        assert!(matches!(
            &terms[3],
            Expr::Condition(Condition {
                operator: Operator::Compare(Comparison::Eq, Literal::Bool(false)),
                ..
            })
        ));
    }

    #[test]
    fn errors_point_at_the_problem() {
        for (text, position, fragment) in [
            ("SELECT FROM custom:x", 8, "expected a name"),
            (
                "SELECT name FROM custom:x WHERE",
                32,
                "the end of the query",
            ),
            (
                "SELECT name FROM custom:x WHERE name = 'open",
                40,
                "not closed",
            ),
            ("SELECT name FROM custom:x WHERE name ~ 1", 38, "'~'"),
            ("SELECT name FROM custom:x LIMIT 0", 33, "LIMIT"),
            (
                "SELECT name FROM custom:x WHERE name NOT = 1",
                42,
                "IN or LIKE",
            ),
            ("SELECT SUM() FROM custom:x", 8, "needs a field"),
            (
                "SELECT name FROM custom:x extra",
                27,
                "the end of the query",
            ),
            (
                "SELECT (SELECT name FROM custom:y) FROM custom:x",
                8,
                "child subqueries",
            ),
            (
                "SELECT name FROM custom:x WHERE d = LAST_N_DAYS:1.5",
                49,
                "whole number",
            ),
            (
                "SELECT name FROM custom:x WHERE d = 2026-13-01",
                37,
                "not a date",
            ),
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
}
