//! 宏表与条件表达式求值（MACRO_SPEC §4）。
//!
//! 递归下降实现，文法：
//! `or ::= and { "||" and }`，`and ::= eq { "&&" eq }`，`eq ::= rel {("=="|"!=") rel}`，
//! `rel ::= unary {("<"|">"|"<="|">=") unary}`，`unary ::= "!" unary | primary`，
//! `primary ::= "(" expr ")" | "defined(NAME)" | NAME | integer`。

use std::collections::HashMap;

/// 宏的绑定值（SPEC §4.2：Defined 视为 1，Str 视为 0，Num 取整数值）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroValue {
    Defined,
    Num(i64),
    Str(String),
}

impl MacroValue {
    /// 表达式中该宏的数值（SPEC §4.1 primary 规则）。
    pub fn as_num(&self) -> i64 {
        match self {
            MacroValue::Defined => 1,
            MacroValue::Num(n) => *n,
            MacroValue::Str(_) => 0,
        }
    }
}

/// 文件内有效的宏表。初始内容来自 CLI（-D/-U），文件内 #define/#undef 顺序生效。
#[derive(Debug, Clone, Default)]
pub struct MacroTable {
    map: HashMap<String, MacroValue>,
}

impl MacroTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn define(&mut self, name: &str, value: MacroValue) {
        self.map.insert(name.to_string(), value);
    }

    pub fn undef(&mut self, name: &str) {
        self.map.remove(name);
    }

    pub fn get(&self, name: &str) -> Option<&MacroValue> {
        self.map.get(name)
    }

    fn lookup_num(&self, name: &[u8]) -> i64 {
        // 输入来自字节流，标识符已保证是 ASCII 名字（scanner 只放行 [A-Za-z0-9_]）。
        std::str::from_utf8(name)
            .ok()
            .and_then(|n| self.map.get(n))
            .map(|v| v.as_num())
            .unwrap_or(0) // 未定义标识符按 0（SPEC §8 差异 6，与 cpp 一致）
    }
}

/// 表达式求值错误（→ E7 BadExpression）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalError {
    /// 相对于表达式起点（不含指令关键字）的字节偏移。
    pub pos: usize,
    pub message: String,
}

impl EvalError {
    fn new(pos: usize, message: impl Into<String>) -> Self {
        Self {
            pos,
            message: message.into(),
        }
    }
}

// 限制括号与 ! 的累计嵌套，超深输入走 E7，避免递归耗尽线程栈。
const MAX_EXPR_DEPTH: usize = 128;

struct Parser<'t> {
    src: &'t [u8],
    pos: usize,
    macros: &'t MacroTable,
    depth: usize,
}

/// 求值表达式，返回整数值（调用方按“非零为真”解释）。
pub fn eval_expr(src: &[u8], macros: &MacroTable) -> Result<i64, EvalError> {
    let mut p = Parser {
        src,
        pos: 0,
        macros,
        depth: 0,
    };
    let v = p.parse_or()?;
    p.skip_ws();
    if p.pos != p.src.len() {
        return Err(p.err("unexpected trailing input"));
    }
    Ok(v)
}

impl<'t> Parser<'t> {
    fn err(&self, msg: impl Into<String>) -> EvalError {
        EvalError::new(self.pos, msg)
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn eat(&mut self, tok: &[u8]) -> bool {
        self.skip_ws();
        if self.src[self.pos..].starts_with(tok) {
            self.pos += tok.len();
            true
        } else {
            false
        }
    }

    fn peek2(&mut self, a: u8, b: u8) -> bool {
        self.skip_ws();
        self.src.get(self.pos) == Some(&a) && self.src.get(self.pos + 1) == Some(&b)
    }

    fn parse_or(&mut self) -> Result<i64, EvalError> {
        let mut v = self.parse_and()?;
        while self.eat(b"||") {
            let r = self.parse_and()?;
            v = i64::from(v != 0 || r != 0);
        }
        Ok(v)
    }

    fn parse_and(&mut self) -> Result<i64, EvalError> {
        let mut v = self.parse_eq()?;
        while self.eat(b"&&") {
            let r = self.parse_eq()?;
            v = i64::from(v != 0 && r != 0);
        }
        Ok(v)
    }

    fn parse_eq(&mut self) -> Result<i64, EvalError> {
        let mut v = self.parse_rel()?;
        loop {
            if self.eat(b"==") {
                v = i64::from(v == self.parse_rel()?);
            } else if self.eat(b"!=") {
                v = i64::from(v != self.parse_rel()?);
            } else {
                return Ok(v);
            }
        }
    }

    fn parse_rel(&mut self) -> Result<i64, EvalError> {
        let mut v = self.parse_unary()?;
        loop {
            if self.eat(b"<=") {
                v = i64::from(v <= self.parse_unary()?);
            } else if self.eat(b">=") {
                v = i64::from(v >= self.parse_unary()?);
            } else if self.peek2(b'<', b'<') || self.peek2(b'>', b'>') {
                // 防止把 "<<" 误读为 "<"
                return Err(self.err("operator not supported"));
            } else if self.eat(b"<") {
                v = i64::from(v < self.parse_unary()?);
            } else if self.eat(b">") {
                v = i64::from(v > self.parse_unary()?);
            } else {
                return Ok(v);
            }
        }
    }

    fn enter_nested(&mut self) -> Result<(), EvalError> {
        if self.depth == MAX_EXPR_DEPTH {
            return Err(self.err("expression nesting exceeds 128 levels"));
        }
        self.depth += 1;
        Ok(())
    }

    fn parse_unary(&mut self) -> Result<i64, EvalError> {
        self.skip_ws();
        if self.eat(b"!") {
            self.enter_nested()?;
            let v = self.parse_unary();
            self.depth -= 1;
            return Ok(i64::from(v? == 0));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<i64, EvalError> {
        self.skip_ws();
        match self.src.get(self.pos) {
            None => Err(self.err("unexpected end of expression")),
            Some(b'(') => {
                self.pos += 1;
                self.enter_nested()?;
                let v = self.parse_or();
                self.depth -= 1;
                let v = v?;
                if !self.eat(b")") {
                    return Err(self.err("expected ')'"));
                }
                Ok(v)
            }
            Some(c) if c.is_ascii_digit() => self.parse_int(),
            Some(c) if c.is_ascii_alphabetic() || *c == b'_' => self.parse_ident(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn parse_int(&mut self) -> Result<i64, EvalError> {
        let start = self.pos;
        while self.src.get(self.pos).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        std::str::from_utf8(&self.src[start..self.pos])
            .unwrap()
            .parse::<i64>()
            .map_err(|_| self.err("integer out of range"))
    }

    fn parse_ident(&mut self) -> Result<i64, EvalError> {
        let start = self.pos;
        self.pos += 1; // 首字符已验证
        while self
            .src
            .get(self.pos)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.pos += 1;
        }
        let ident = &self.src[start..self.pos];
        if ident == b"defined" {
            self.skip_ws();
            if !self.eat(b"(") {
                return Err(self.err("expected '(' after defined"));
            }
            let name = self.parse_ident_name()?;
            self.skip_ws();
            if !self.eat(b")") {
                return Err(self.err("expected ')' after defined(NAME)"));
            }
            return Ok(i64::from(
                self.macros
                    .get(std::str::from_utf8(name).unwrap())
                    .is_some(),
            ));
        }
        Ok(self.macros.lookup_num(ident))
    }

    /// 解析标识符并返回字节切片（供 defined(NAME) 与普通 NAME 共用）。
    fn parse_ident_name(&mut self) -> Result<&'t [u8], EvalError> {
        self.skip_ws();
        let start = self.pos;
        match self.src.get(self.pos) {
            Some(c) if c.is_ascii_alphabetic() || *c == b'_' => self.pos += 1,
            _ => return Err(self.err("expected identifier")),
        }
        while self
            .src
            .get(self.pos)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.pos += 1;
        }
        Ok(&self.src[start..self.pos])
    }
}

/// 校验宏名文法：[A-Za-z_][A-Za-z0-9_]*（SPEC §3）。
pub fn is_valid_name(name: &[u8]) -> bool {
    let valid =
        |c: u8, first: bool| c.is_ascii_alphabetic() || c == b'_' || (!first && c.is_ascii_digit());
    match name.split_first() {
        Some((c, rest)) => valid(*c, true) && rest.iter().all(|c| valid(*c, false)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(pairs: &[(&str, MacroValue)]) -> MacroTable {
        let mut t = MacroTable::new();
        for (n, v) in pairs {
            t.define(n, v.clone());
        }
        t
    }

    fn eval(src: &str, t: &MacroTable) -> i64 {
        eval_expr(src.as_bytes(), t).unwrap()
    }

    fn eval_err(src: &str, t: &MacroTable) -> EvalError {
        eval_expr(src.as_bytes(), t).unwrap_err()
    }

    #[test]
    fn literals_and_precedence() {
        let t = table(&[]);
        assert_eq!(eval("1", &t), 1);
        assert!(eval_expr(b"2 + 3", &t).is_err()); // 无算术运算（V1）
        assert_eq!(eval("1 || 0", &t), 1);
        assert_eq!(eval("1 && 0", &t), 0);
        // && 高于 ||
        assert_eq!(eval("1 || 0 && 0", &t), 1);
        assert_eq!(eval("(1 || 0) && 0", &t), 0);
        // 比较高于 &&
        assert_eq!(eval("2 > 1 && 3 >= 3", &t), 1);
        assert_eq!(eval("1 == 1 && 2 != 3", &t), 1);
        assert_eq!(eval("!0", &t), 1);
        assert_eq!(eval("!!5", &t), 1);
        assert_eq!(eval("!1 && !0", &t), 0);
    }

    #[test]
    fn short_circuit_keeps_semantics() {
        let t = table(&[]);
        // 短路在结果上等价（V1 表达式无副作用）
        assert_eq!(eval("0 && 1", &t), 0);
        assert_eq!(eval("1 || (0 == 0)", &t), 1);
    }

    #[test]
    fn defined_and_undefined_idents() {
        let t = table(&[
            ("RELEASE", MacroValue::Defined),
            ("LEVEL", MacroValue::Num(3)),
            ("TAG", MacroValue::Str("prod".into())),
        ]);
        assert_eq!(eval("defined(RELEASE)", &t), 1);
        assert_eq!(eval("defined ( LEVEL )", &t), 1);
        assert_eq!(eval("!defined(MISSING)", &t), 1);
        // 未定义标识符 → 0（与 cpp 一致）
        assert_eq!(eval("MISSING", &t), 0);
        assert_eq!(eval("MISSING || 1", &t), 1);
        // Defined→1, Num→n, Str→0
        assert_eq!(eval("RELEASE", &t), 1);
        assert_eq!(eval("LEVEL", &t), 3);
        assert_eq!(eval("LEVEL == 3", &t), 1);
        assert_eq!(eval("TAG", &t), 0);
        assert_eq!(eval("TAG && 0", &t), 0);
    }

    #[test]
    fn malformed_exprs_are_e7() {
        let t = table(&[]);
        assert!(eval_err("", &t).message.contains("end of expression"));
        assert!(eval_err("1 +", &t).message.contains("unexpected"));
        assert!(eval_err("(1", &t).message.contains("')'"));
        assert!(eval_err("1)", &t).message.contains("trailing"));
        assert!(eval_err("defined RELEASE", &t).message.contains("'('"));
        assert!(eval_err("defined(X", &t).message.contains("')'"));
        assert!(eval_err("99999999999999999999", &t)
            .message
            .contains("range"));
        assert!(eval_err("1 << 2", &t).message.contains("not supported"));
        assert!(eval_err("#", &t).message.contains("unexpected character"));
    }

    #[test]
    fn name_validation() {
        assert!(is_valid_name(b"RELEASE"));
        assert!(is_valid_name(b"_x"));
        assert!(is_valid_name(b"V2"));
        assert!(!is_valid_name(b"2V"));
        assert!(!is_valid_name(b"a-b"));
        assert!(!is_valid_name(b""));
    }
}
