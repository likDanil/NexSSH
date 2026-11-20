//! Read literal data from the original UI; never execute TypeScript at build time.
use serde_json::{Map, Value};
use std::{collections::BTreeMap, fs, path::Path};

struct Literals<'a> {
    source: &'a str,
    offset: usize,
}
impl<'a> Literals<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, offset: 0 }
    }
    fn rest(&self) -> &'a str {
        &self.source[self.offset..]
    }
    fn whitespace(&mut self) {
        loop {
            while self.rest().starts_with(char::is_whitespace) {
                self.offset += self.rest().chars().next().unwrap().len_utf8();
            }
            if self.rest().starts_with("//") {
                self.offset += self.rest().find('\n').unwrap_or(self.rest().len());
            } else if self.rest().starts_with("/*") {
                self.offset += self.rest().find("*/").expect("closed comment") + 2;
            } else {
                break;
            }
        }
    }
    fn take(&mut self, token: &str) {
        self.whitespace();
        assert!(
            self.rest().starts_with(token),
            "expected {token}: {}",
            self.rest()
        );
        self.offset += token.len();
    }
    fn string(&mut self) -> String {
        self.whitespace();
        let quote = self.rest().chars().next().unwrap();
        assert!(quote == '\'' || quote == '"');
        self.offset += 1;
        let mut out = String::new();
        loop {
            let ch = self.rest().chars().next().expect("closed string");
            self.offset += ch.len_utf8();
            if ch == quote {
                break;
            }
            if ch != '\\' {
                out.push(ch);
                continue;
            }
            let ch = self.rest().chars().next().unwrap();
            self.offset += ch.len_utf8();
            out.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '\'' => '\'',
                '"' => '"',
                other => other,
            });
        }
        out
    }
    fn identifier(&mut self) -> String {
        self.whitespace();
        let len = self
            .rest()
            .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .unwrap_or(self.rest().len());
        assert!(len > 0);
        let value = self.rest()[..len].to_owned();
        self.offset += len;
        value
    }
    fn value(&mut self) -> Value {
        self.whitespace();
        match self.rest().chars().next().unwrap() {
            '\'' | '"' => Value::String(self.string()),
            '{' => {
                self.take("{");
                let mut map = Map::new();
                loop {
                    self.whitespace();
                    if self.rest().starts_with('}') {
                        self.take("}");
                        break;
                    }
                    let key = if self.rest().starts_with(['\'', '"']) {
                        self.string()
                    } else {
                        self.identifier()
                    };
                    self.take(":");
                    map.insert(key, self.value());
                    self.whitespace();
                    if self.rest().starts_with(',') {
                        self.take(",");
                    }
                }
                Value::Object(map)
            }
            '[' => {
                self.take("[");
                let mut values = vec![];
                loop {
                    self.whitespace();
                    if self.rest().starts_with(']') {
                        self.take("]");
                        break;
                    }
                    values.push(self.value());
                    self.whitespace();
                    if self.rest().starts_with(',') {
                        self.take(",");
                    }
                }
                Value::Array(values)
            }
            _ => {
                let len = self
                    .rest()
                    .find(|ch: char| ch.is_whitespace() || [',', '}', ']'].contains(&ch))
                    .unwrap();
                let word = &self.rest()[..len];
                self.offset += len;
                serde_json::from_str(word).expect("JSON-compatible literal")
            }
        }
    }
}

fn locale(path: &str) -> BTreeMap<String, String> {
    println!("cargo:rerun-if-changed={path}");
    let text = fs::read_to_string(path).unwrap();
    let source = text.split_once("= {").expect("locale object").1;
    let object = Literals::new(&format!("{{{source}")).value();
    object
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(key, value)| {
            value
                .as_str()
                .or_else(|| value.get("other").and_then(Value::as_str))
                .map(|text| (key.clone(), text.to_owned()))
        })
        .collect()
}

pub fn generate(out: &Path) {
    let highlight = "../ui/src/lib/highlight.ts";
    println!("cargo:rerun-if-changed={highlight}");
    let source = fs::read_to_string(highlight).unwrap();
    let words = source
        .split_once("const WORDS:")
        .unwrap()
        .1
        .split_once("= {")
        .unwrap()
        .1;
    let words = Literals::new(&format!("{{{words}")).value();
    let mut patterns = String::from("pub const WORDS: &[(&str,&str)] = &[\n");
    for kind in ["error", "warning", "ok", "info", "debug"] {
        patterns.push_str(&format!(
            "({kind:?},{:?}),\n",
            words[kind].as_str().unwrap()
        ));
    }
    patterns.push_str("];\n");
    fs::write(out.join("highlight_words.rs"), patterns).unwrap();
    let path = "../ui/src/lib/state/app.svelte.ts";
    println!("cargo:rerun-if-changed={path}");
    let text = fs::read_to_string(path).unwrap();
    let defaults = text
        .split_once("export const DEFAULT_SETTINGS: Settings = ")
        .unwrap()
        .1;
    let value = Literals::new(defaults).value();
    fs::write(
        out.join("settings_defaults.json"),
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();
    let en = locale("../ui/src/lib/locales/en.ts");
    let ru = locale("../ui/src/lib/locales/ru.ts");
    let mut rust = String::from("pub const TRANSLATIONS: &[(&str, &str, &str)] = &[\n");
    for (key, english) in &en {
        rust.push_str(&format!(
            "({key:?},{english:?},{:?}),\n",
            ru.get(key).unwrap_or(english)
        ));
    }
    rust.push_str("];\n");
    fs::write(out.join("locales.rs"), rust).unwrap();
    assert!(en.contains_key("settings.general") && ru.contains_key("settings.general"));
}
