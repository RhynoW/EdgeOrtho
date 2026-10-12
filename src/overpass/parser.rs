//! Overpass QL 子集解析器（見 docs/overpass-duckdb-design.md 第 3 節）。
//! 支援：`[out:json][timeout:N][bbox:s,w,n,e]`、`node|way|rel|relation|nwr` 查詢加標籤過濾與 `(s,w,n,e)`、
//! 聯集 `( ...; );`、`out [body|tags|geom|center|ids|meta] [qt] [N];`。其餘語法一律回錯誤，不靜默忽略。

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Exists,
    NotExists,
    Eq(String),
    Ne(String),
    Re(String, bool),
    NotRe(String, bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub key: String,
    pub op: Op,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub types: Vec<&'static str>,
    pub filters: Vec<Filter>,
    pub bbox: Option<[f64; 4]>, // s, w, n, e
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutMode {
    Body,
    Geom,
    Center,
    Ids,
}

#[derive(Debug)]
pub struct Program {
    pub timeout: Option<u64>,
    pub bbox: Option<[f64; 4]>,
    pub queries: Vec<Query>,
    pub out: OutMode,
    pub limit: Option<usize>,
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

type R<T> = Result<T, String>;

impl<'a> P<'a> {
    fn ws(&mut self) {
        loop {
            while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
            if self.s[self.i.min(self.s.len())..].starts_with(b"//") {
                while self.i < self.s.len() && self.s[self.i] != b'\n' {
                    self.i += 1;
                }
            } else if self.s[self.i.min(self.s.len())..].starts_with(b"/*") {
                match find(&self.s[self.i..], b"*/") {
                    Some(n) => self.i += n + 2,
                    None => self.i = self.s.len(),
                }
            } else {
                return;
            }
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, c: u8) -> R<()> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(format!("expected '{}' at offset {}", c as char, self.i))
        }
    }
    fn word(&mut self) -> String {
        self.ws();
        let st = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || b"_:.-".contains(&self.s[self.i])) {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[st..self.i]).into_owned()
    }
    fn ident(&mut self) -> String {
        self.ws();
        let st = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[st..self.i]).into_owned()
    }
    fn quoted(&mut self) -> R<String> {
        let q = self.s[self.i];
        self.i += 1;
        let mut out = Vec::new();
        while self.i < self.s.len() && self.s[self.i] != q {
            if self.s[self.i] == b'\\' && self.i + 1 < self.s.len() {
                self.i += 1;
            }
            out.push(self.s[self.i]);
            self.i += 1;
        }
        if self.i >= self.s.len() {
            return Err("unterminated string".into());
        }
        self.i += 1;
        String::from_utf8(out).map_err(|e| e.to_string())
    }
    fn str_or_word(&mut self) -> R<String> {
        match self.peek() {
            Some(b'"') | Some(b'\'') => self.quoted(),
            _ => {
                let w = self.word();
                if w.is_empty() {
                    Err(format!("expected a key or value at offset {}", self.i))
                } else {
                    Ok(w)
                }
            }
        }
    }
    fn number(&mut self) -> R<f64> {
        self.ws();
        let st = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || b"+-.eE".contains(&self.s[self.i])) {
            self.i += 1;
        }
        std::str::from_utf8(&self.s[st..self.i])
            .unwrap_or("")
            .parse()
            .map_err(|_| format!("expected a number at offset {st}"))
    }
    fn bbox4(&mut self) -> R<[f64; 4]> {
        let mut v = [0f64; 4];
        for (k, slot) in v.iter_mut().enumerate() {
            if k > 0 {
                self.expect(b',')?;
            }
            *slot = self.number()?;
        }
        if !(v[0] <= v[2] && (-90.0..=90.0).contains(&v[0]) && (-90.0..=90.0).contains(&v[2])) {
            return Err("invalid bbox: expected south,west,north,east with south <= north".into());
        }
        Ok(v)
    }
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn filter(p: &mut P) -> R<Filter> {
    // 已吃掉 '['
    let neg = p.eat(b'!');
    let key = p.str_or_word()?;
    if neg {
        p.expect(b']')?;
        return Ok(Filter { key, op: Op::NotExists });
    }
    p.ws();
    let rest = &p.s[p.i..];
    let op = if rest.starts_with(b"!~") {
        p.i += 2;
        let v = p.str_or_word()?;
        Op::NotRe(v, false)
    } else if rest.starts_with(b"!=") {
        p.i += 2;
        Op::Ne(p.str_or_word()?)
    } else if rest.starts_with(b"~") {
        p.i += 1;
        Op::Re(p.str_or_word()?, false)
    } else if rest.starts_with(b"=") {
        p.i += 1;
        Op::Eq(p.str_or_word()?)
    } else {
        Op::Exists
    };
    let op = if p.eat(b',') {
        let flag = p.word();
        if flag != "i" {
            return Err("only the ',i' regex flag is supported".into());
        }
        match op {
            Op::Re(v, _) => Op::Re(v, true),
            Op::NotRe(v, _) => Op::NotRe(v, true),
            _ => return Err("',i' flag only valid on ~ and !~".into()),
        }
    } else {
        op
    };
    p.expect(b']')?;
    if let Op::Re(v, _) | Op::NotRe(v, _) = &op {
        if v.len() > 128 {
            return Err("regex longer than 128 characters".into());
        }
    }
    Ok(Filter { key, op })
}

fn query(p: &mut P, kw: &str) -> R<Query> {
    let types: Vec<&'static str> = match kw {
        "node" => vec!["node"],
        "way" => vec!["way"],
        "rel" | "relation" => vec!["relation"],
        "nwr" | "nw" => vec!["node", "way", "relation"],
        _ => return Err(format!("unsupported statement '{kw}'")),
    };
    let mut q = Query { types, filters: vec![], bbox: None };
    loop {
        match p.peek() {
            Some(b'[') => {
                p.i += 1;
                q.filters.push(filter(p)?);
            }
            Some(b'(') => {
                p.i += 1;
                p.ws();
                if p.s[p.i..].starts_with(b"area") || p.s[p.i..].starts_with(b"around") || p.s[p.i..].starts_with(b"poly") {
                    return Err("area/around/poly filters are not supported".into());
                }
                q.bbox = Some(p.bbox4()?);
                p.expect(b')')?;
            }
            Some(b'-') => return Err("result-set variables (->.x) are not supported".into()),
            _ => break,
        }
    }
    Ok(q)
}

pub fn parse(src: &str) -> R<Program> {
    let mut p = P { s: src.as_bytes(), i: 0 };
    let mut prog = Program { timeout: None, bbox: None, queries: vec![], out: OutMode::Body, limit: None };
    // 設定 [..][..];
    while p.peek() == Some(b'[') {
        p.i += 1;
        let k = p.ident();
        p.expect(b':')?;
        match k.as_str() {
            "out" => {
                if p.word() != "json" {
                    return Err("only [out:json] is supported".into());
                }
            }
            "timeout" => prog.timeout = Some(p.number()? as u64),
            "bbox" => prog.bbox = Some(p.bbox4()?),
            "maxsize" => {
                p.number()?;
            }
            other => return Err(format!("unsupported setting [{other}:...]")),
        }
        p.expect(b']')?;
    }
    if p.peek().is_some() {
        p.eat(b';');
    }
    let mut out_seen = false;
    while p.peek().is_some() {
        if p.eat(b'(') {
            // 聯集
            while !p.eat(b')') {
                let kw = p.word();
                if kw.is_empty() {
                    return Err(format!("unexpected input in union at offset {}", p.i));
                }
                prog.queries.push(query(&mut p, &kw)?);
                p.expect(b';')?;
            }
            p.expect(b';')?;
            continue;
        }
        let kw = p.word();
        if kw == "out" {
            out_seen = true;
            loop {
                p.ws();
                if p.eat(b';') {
                    break;
                }
                let w = p.word();
                match w.as_str() {
                    "body" | "tags" | "meta" | "skel" | "qt" | "asc" => {}
                    "geom" => prog.out = OutMode::Geom,
                    "center" => prog.out = OutMode::Center,
                    "ids" => prog.out = OutMode::Ids,
                    n if n.parse::<usize>().is_ok() => prog.limit = n.parse().ok(),
                    _ => return Err(format!("unsupported out option '{w}'")),
                }
            }
            continue;
        }
        if kw.is_empty() {
            return Err(format!("unexpected character at offset {}", p.i));
        }
        prog.queries.push(query(&mut p, &kw)?);
        p.expect(b';')?;
    }
    if prog.queries.is_empty() {
        return Err("no query statement".into());
    }
    if !out_seen {
        return Err("missing 'out' statement".into());
    }
    Ok(prog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pikmin_style() {
        let p = parse(r#"[out:json][timeout:5][bbox:25.0,121.5,25.1,121.6];(nwr["leisure"~"park|garden"];nwr[natural=wood];);out geom;"#).unwrap();
        assert_eq!(p.timeout, Some(5));
        assert_eq!(p.bbox, Some([25.0, 121.5, 25.1, 121.6]));
        assert_eq!(p.queries.len(), 2);
        assert_eq!(p.queries[0].filters[0].op, Op::Re("park|garden".into(), false));
        assert_eq!(p.queries[1].filters[0].op, Op::Eq("wood".into()));
        assert_eq!(p.out, OutMode::Geom);
    }
    #[test]
    fn per_statement_bbox_and_flags() {
        let p = parse(r#"node["name"~"cafe",i][!shop][amenity!=bank](25,121,25.5,121.5);out center 10;"#).unwrap();
        assert_eq!(p.queries[0].bbox, Some([25.0, 121.0, 25.5, 121.5]));
        assert_eq!(p.queries[0].filters.len(), 3);
        assert_eq!(p.queries[0].filters[0].op, Op::Re("cafe".into(), true));
        assert_eq!(p.queries[0].filters[1].op, Op::NotExists);
        assert_eq!(p.out, OutMode::Center);
        assert_eq!(p.limit, Some(10));
    }
    #[test]
    fn rejects_unsupported() {
        assert!(parse("node(around:100,25,121);out;").is_err());
        assert!(parse("way[a=b];>;out;").is_err());
        assert!(parse("[out:xml];node[a=b](1,2,3,4);out;").is_err());
        assert!(parse("node[a=b](1,2,3,4);").is_err());
        assert!(parse("node(area.a);out;").is_err());
    }
    #[test]
    fn comments_and_quotes() {
        let p = parse("// c\nnode[\"name:en\"=\"A \\\"B\\\"\"](1,2,3,4); /* x */ out;").unwrap();
        assert_eq!(p.queries[0].filters[0].key, "name:en");
        assert_eq!(p.queries[0].filters[0].op, Op::Eq("A \"B\"".into()));
    }
}
