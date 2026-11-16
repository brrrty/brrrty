//! PostHTML as the RSS plugin and 11ty's HTML transformer run it: parse
//! with `posthtml-parser` 0.11.0 (`htmlparser2` 7.2.0 with
//! `{ lowerCaseTags: false, lowerCaseAttributeNames: false,
//! decodeEntities: false }`), change the tree, and print it with
//! `posthtml-render` 3.0.0.
//!
//! This is not an HTML5 parser, and it is not meant to be one: it gives the
//! tree those libraries give, quirks included. Tag and attribute names keep
//! their case and are compared as written (`<IMG>` is not a void element to
//! the parser), entities stay as written, a `<p>` is closed by the block
//! tags `htmlparser2` lists, `<?xml ...?>` and declarations other than
//! `<!doctype>` are dropped, an unclosed tag at the end of the text is
//! dropped, and every attribute is printed in double quotes (a value that
//! looks like JSON in single quotes), so `disabled` becomes `disabled=""`.

/// A node of the tree, as `posthtml-parser` makes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// Text, a comment (`<!--x-->`) or a doctype, as written.
    Text(String),
    Element(Element),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub tag: String,
    /// In JavaScript's property order: integer-like names first, in
    /// numeric order, then the rest as written. The first of two
    /// attributes with one name wins. `None` when the tag has none.
    pub attrs: Option<Vec<(String, String)>>,
    pub content: Vec<Node>,
}

/// How `posthtml-render` closes a void element (`closingSingleTag`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closing {
    /// `<br>`: the default, which 11ty's HTML transformer uses.
    Default,
    /// `<br />`: what `htmlToAbsoluteUrls` asks for.
    Slash,
}

const VOID: [&str; 19] = [
    "area", "base", "basefont", "br", "col", "command", "embed", "frame", "hr", "img", "input",
    "isindex", "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// `posthtml-render`'s `singleTags`, compared lowercased.
const SINGLE: [&str; 17] = [
    "area", "base", "br", "col", "command", "embed", "hr", "img", "input", "keygen", "link",
    "menuitem", "meta", "param", "source", "track", "wbr",
];

const FOREIGN: [&str; 2] = ["math", "svg"];
const INTEGRATION: [&str; 9] = [
    "mi",
    "mo",
    "mn",
    "ms",
    "mtext",
    "annotation-xml",
    "foreignobject",
    "desc",
    "title",
];

/// `openImpliesClose`: opening the key closes these while they are on top.
fn implies_close(name: &str) -> &'static [&'static str] {
    const FORM: &[&str] = &[
        "input", "option", "optgroup", "select", "button", "datalist", "textarea",
    ];
    const P: &[&str] = &["p"];
    const DDT: &[&str] = &["dd", "dt"];
    const RTP: &[&str] = &["rt", "rp"];
    const SECTION: &[&str] = &["thead", "tbody"];
    match name {
        "tr" => &["tr", "th", "td"],
        "th" => &["th"],
        "td" => &["thead", "th", "td"],
        "body" => &["head", "link", "script"],
        "li" => &["li"],
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => P,
        "select" | "input" | "output" | "button" | "datalist" | "textarea" => FORM,
        "option" => &["option"],
        "optgroup" => &["optgroup", "option"],
        "dd" | "dt" => DDT,
        "address" | "article" | "aside" | "blockquote" | "details" | "div" | "dl" | "fieldset"
        | "figcaption" | "figure" | "footer" | "form" | "header" | "hr" | "main" | "nav" | "ol"
        | "pre" | "section" | "table" | "ul" => P,
        "rt" | "rp" => RTP,
        "tbody" | "tfoot" => SECTION,
        _ => &[],
    }
}

fn is_void(name: &str) -> bool {
    VOID.contains(&name)
}

/// The tree builder: `posthtml-parser`'s callbacks behind `htmlparser2`'s
/// `Parser`.
struct Builder {
    // htmlparser2's Parser state.
    tagname: String,
    attribname: String,
    attribvalue: String,
    attribs: Option<Vec<(String, String)>>,
    stack: Vec<String>,
    foreign: Vec<bool>,
    // posthtml-parser's state.
    open: Vec<Element>,
    root: Vec<Node>,
}

impl Builder {
    fn new() -> Builder {
        Builder {
            tagname: String::new(),
            attribname: String::new(),
            attribvalue: String::new(),
            attribs: None,
            stack: Vec::new(),
            foreign: Vec::new(),
            open: Vec::new(),
            root: Vec::new(),
        }
    }

    // ---- posthtml-parser ----

    fn push_node(&mut self, node: Node) {
        match self.open.last_mut() {
            Some(e) => e.content.push(node),
            None => self.root.push(node),
        }
    }

    fn cb_text(&mut self, text: &str) {
        match self.open.last_mut() {
            None => self.root.push(Node::Text(text.to_string())),
            Some(e) => {
                if let Some(Node::Text(last)) = e.content.last_mut()
                    && !last.starts_with("<!--")
                {
                    last.push_str(text);
                    return;
                }
                e.content.push(Node::Text(text.to_string()));
            }
        }
    }

    fn cb_comment(&mut self, text: &str) {
        self.push_node(Node::Text(format!("<!--{text}-->")));
    }

    fn cb_processing_instruction(&mut self, name: &str, data: &str) {
        // The only directive is `<!doctype ...>`, matched case-insensitively.
        if name.to_lowercase() == "!doctype" {
            self.push_node(Node::Text(format!("<{data}>")));
        }
    }

    fn cb_open_tag(&mut self, name: &str, attrs: Vec<(String, String)>) {
        let attrs = (!attrs.is_empty()).then(|| {
            // `posthtml-parser` turns `&quot;` back into `"`.
            attrs
                .into_iter()
                .map(|(k, v)| (k, v.replace("&quot;", "\"")))
                .collect()
        });
        self.open.push(Element {
            tag: name.to_string(),
            attrs,
            content: Vec::new(),
        });
    }

    fn cb_close_tag(&mut self) {
        if let Some(e) = self.open.pop() {
            self.push_node(Node::Element(e));
        }
    }

    // ---- htmlparser2 Parser ----

    fn onopentagname(&mut self, name: &str) {
        self.emit_open_tag(name);
    }

    fn emit_open_tag(&mut self, name: &str) {
        self.tagname = name.to_string();
        let closes = implies_close(name);
        while let Some(top) = self.stack.last() {
            if !closes.contains(&top.as_str()) {
                break;
            }
            self.stack.pop();
            self.cb_close_tag();
        }
        if !is_void(name) {
            self.stack.push(name.to_string());
            if FOREIGN.contains(&name) {
                self.foreign.push(true);
            } else if INTEGRATION.contains(&name) {
                self.foreign.push(false);
            }
        }
        self.attribs = Some(Vec::new());
    }

    fn end_open_tag(&mut self) {
        if let Some(attribs) = self.attribs.take() {
            let name = self.tagname.clone();
            self.cb_open_tag(&name, js_key_order(attribs));
        }
        if is_void(&self.tagname) {
            self.cb_close_tag();
        }
        self.tagname.clear();
    }

    fn onopentagend(&mut self) {
        self.end_open_tag();
    }

    fn onclosetag(&mut self, name: &str) {
        if FOREIGN.contains(&name) || INTEGRATION.contains(&name) {
            self.foreign.pop();
        }
        if !is_void(name) {
            if let Some(pos) = self.stack.iter().rposition(|s| s == name) {
                for _ in pos..self.stack.len() {
                    self.stack.pop();
                    self.cb_close_tag();
                }
            } else if name == "p" {
                self.emit_open_tag(name);
                self.close_current_tag();
            }
        } else if name == "br" {
            self.cb_open_tag("br", Vec::new());
            self.cb_close_tag();
        }
    }

    fn onselfclosingtag(&mut self) {
        if self.foreign.last() == Some(&true) {
            self.close_current_tag();
        } else {
            self.onopentagend();
        }
    }

    fn close_current_tag(&mut self) {
        let name = self.tagname.clone();
        self.end_open_tag();
        if self.stack.last() == Some(&name) {
            self.cb_close_tag();
            self.stack.pop();
        }
    }

    fn onattribname(&mut self, name: &str) {
        self.attribname = name.to_string();
    }

    fn onattribdata(&mut self, value: &str) {
        self.attribvalue.push_str(value);
    }

    fn onattribend(&mut self) {
        let name = std::mem::take(&mut self.attribname);
        let value = std::mem::take(&mut self.attribvalue);
        // `attribs.__proto__ = "x"` sets nothing in JavaScript.
        if let Some(a) = &mut self.attribs
            && name != "__proto__"
            && !a.iter().any(|(k, _)| *k == name)
        {
            a.push((name, value));
        }
    }

    fn instruction_name(value: &str) -> &str {
        let end = value
            .char_indices()
            .find(|&(_, c)| js_space(c) || c == '/')
            .map_or(value.len(), |(i, _)| i);
        &value[..end]
    }

    fn ondeclaration(&mut self, value: &str) {
        let name = Self::instruction_name(value);
        self.cb_processing_instruction(&format!("!{name}"), &format!("!{value}"));
    }

    fn onprocessinginstruction(&mut self, value: &str) {
        let name = Self::instruction_name(value);
        self.cb_processing_instruction(&format!("?{name}"), &format!("?{value}"));
    }

    fn oncdata(&mut self, value: &str) {
        self.cb_comment(&format!("[CDATA[{value}]]"));
    }

    fn onend(&mut self) {
        for _ in 0..self.stack.len() {
            self.cb_close_tag();
        }
    }
}

pub(crate) use brrrty_js::is_js_whitespace as js_space;

fn js_key_order(attrs: Vec<(String, String)>) -> Vec<(String, String)> {
    let (mut index, rest): (Vec<_>, Vec<_>) = attrs
        .into_iter()
        .partition(|(k, _)| brrrty_js::array_index(k).is_some());
    index.sort_by_key(|(k, _)| brrrty_js::array_index(k));
    index.extend(rest);
    index
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Text,
    BeforeTagName,
    InTagName,
    InSelfClosingTag,
    BeforeClosingTagName,
    InClosingTagName,
    AfterClosingTagName,
    BeforeAttributeName,
    InAttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    InAttributeValueDq,
    InAttributeValueSq,
    InAttributeValueNq,
    BeforeDeclaration,
    InDeclaration,
    InProcessingInstruction,
    BeforeComment,
    CdataSequence,
    InSpecialComment,
    InCommentLike,
    BeforeSpecialS,
    SpecialStartSequence,
    InSpecialTag,
}

const CDATA: &[u8] = b"CDATA[";
const CDATA_END: &[u8] = b"]]>";
const COMMENT_END: &[u8] = b"-->";
const SCRIPT_END: &[u8] = b"</script";
const STYLE_END: &[u8] = b"</style";
const TITLE_END: &[u8] = b"</title";

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\n' | b'\t' | 0x0c | b'\r')
}

fn is_end_of_tag_section(c: u8) -> bool {
    c == b'/' || c == b'>' || is_ws(c)
}

/// `htmlparser2`'s `Tokenizer` with `decodeEntities: false`, outside XML
/// mode, fed the whole text at once. It works on bytes: every character it
/// looks for is ASCII, and no byte of a longer UTF-8 character is ASCII.
struct Tokenizer<'a> {
    buf: &'a [u8],
    text: &'a str,
    state: State,
    section_start: isize,
    index: usize,
    is_special: bool,
    sequence: &'static [u8],
    sequence_index: usize,
}

impl<'a> Tokenizer<'a> {
    fn section(&self) -> &'a str {
        let start = self.section_start.max(0) as usize;
        let end = self.index.max(start);
        &self.text[start.min(self.text.len())..end.min(self.text.len())]
    }

    fn slice(&self, start: usize, end: usize) -> &'a str {
        if end <= start {
            return "";
        }
        &self.text[start..end]
    }

    fn fast_forward_to(&mut self, c: u8) -> bool {
        loop {
            self.index += 1;
            if self.index >= self.buf.len() {
                break;
            }
            if self.buf[self.index] == c {
                return true;
            }
        }
        self.index = self.buf.len() - 1;
        false
    }

    fn state_text(&mut self, b: &mut Builder, c: u8) {
        if c == b'<' || self.fast_forward_to(b'<') {
            if self.index as isize > self.section_start {
                b.cb_text(self.section());
            }
            self.state = State::BeforeTagName;
            self.section_start = self.index as isize;
        }
    }

    fn state_special_start_sequence(&mut self, b: &mut Builder, c: u8) {
        let is_end = self.sequence_index == self.sequence.len();
        let is_match = if is_end {
            is_end_of_tag_section(c)
        } else {
            (c | 0x20) == self.sequence[self.sequence_index]
        };
        if !is_match {
            self.is_special = false;
        } else if !is_end {
            self.sequence_index += 1;
            return;
        }
        self.sequence_index = 0;
        self.state = State::InTagName;
        self.state_in_tag_name(b, c);
    }

    fn state_in_special_tag(&mut self, b: &mut Builder, c: u8) {
        if self.sequence_index == self.sequence.len() {
            if c == b'>' || is_ws(c) {
                let end_of_text = self.index - self.sequence.len();
                if (self.section_start) < end_of_text as isize {
                    let start = self.section_start.max(0) as usize;
                    b.cb_text(self.slice(start, end_of_text));
                }
                self.is_special = false;
                self.section_start = end_of_text as isize + 2;
                self.state_in_closing_tag_name(b, c);
                return;
            }
            self.sequence_index = 0;
        }
        if (c | 0x20) == self.sequence[self.sequence_index] {
            self.sequence_index += 1;
        } else if self.sequence_index == 0 {
            if self.sequence == TITLE_END {
                // Entities aren't decoded, so there is nothing to do.
            } else if self.fast_forward_to(b'<') {
                self.sequence_index = 1;
            }
        } else {
            self.sequence_index = usize::from(c == b'<');
        }
    }

    fn state_cdata_sequence(&mut self, b: &mut Builder, c: u8) {
        if c == CDATA[self.sequence_index] {
            self.sequence_index += 1;
            if self.sequence_index == CDATA.len() {
                self.state = State::InCommentLike;
                self.sequence = CDATA_END;
                self.sequence_index = 0;
                self.section_start = self.index as isize + 1;
            }
        } else {
            self.sequence_index = 0;
            self.state = State::InDeclaration;
            self.state_in_declaration(b, c);
        }
    }

    fn state_in_comment_like(&mut self, b: &mut Builder, c: u8) {
        if c == self.sequence[self.sequence_index] {
            self.sequence_index += 1;
            if self.sequence_index == self.sequence.len() {
                let start = self.section_start.max(0) as usize;
                let section = self.slice(start, self.index.saturating_sub(2));
                if self.sequence == CDATA_END {
                    b.oncdata(section);
                } else {
                    b.cb_comment(section);
                }
                self.sequence_index = 0;
                self.section_start = self.index as isize + 1;
                self.state = State::Text;
            }
        } else if self.sequence_index == 0 {
            if self.fast_forward_to(self.sequence[0]) {
                self.sequence_index = 1;
            }
        } else if c != self.sequence[self.sequence_index - 1] {
            self.sequence_index = 0;
        }
    }

    fn start_special(&mut self, sequence: &'static [u8], offset: usize) {
        self.is_special = true;
        self.sequence = sequence;
        self.sequence_index = offset;
        self.state = State::SpecialStartSequence;
    }

    fn state_before_tag_name(&mut self, b: &mut Builder, c: u8) {
        if c == b'!' {
            self.state = State::BeforeDeclaration;
            self.section_start = self.index as isize + 1;
        } else if c == b'?' {
            self.state = State::InProcessingInstruction;
            self.section_start = self.index as isize + 1;
        } else if c.is_ascii_alphabetic() {
            let lower = c | 0x20;
            self.section_start = self.index as isize;
            if lower == TITLE_END[2] {
                self.start_special(TITLE_END, 3);
            } else {
                self.state = if lower == SCRIPT_END[2] {
                    State::BeforeSpecialS
                } else {
                    State::InTagName
                };
            }
        } else if c == b'/' {
            self.state = State::BeforeClosingTagName;
        } else {
            self.state = State::Text;
            self.state_text(b, c);
        }
    }

    fn state_in_tag_name(&mut self, b: &mut Builder, c: u8) {
        if is_end_of_tag_section(c) {
            b.onopentagname(self.section());
            self.section_start = -1;
            self.state = State::BeforeAttributeName;
            self.state_before_attribute_name(b, c);
        }
    }

    fn state_before_closing_tag_name(&mut self, c: u8) {
        if is_ws(c) {
        } else if c == b'>' {
            self.state = State::Text;
        } else {
            self.state = if c.is_ascii_alphabetic() {
                State::InClosingTagName
            } else {
                State::InSpecialComment
            };
            self.section_start = self.index as isize;
        }
    }

    fn state_in_closing_tag_name(&mut self, b: &mut Builder, c: u8) {
        if c == b'>' || is_ws(c) {
            b.onclosetag(self.section());
            self.section_start = -1;
            self.state = State::AfterClosingTagName;
            self.state_after_closing_tag_name(c);
        }
    }

    fn state_after_closing_tag_name(&mut self, c: u8) {
        if c == b'>' || self.fast_forward_to(b'>') {
            self.state = State::Text;
            self.section_start = self.index as isize + 1;
        }
    }

    fn state_before_attribute_name(&mut self, b: &mut Builder, c: u8) {
        if c == b'>' {
            b.onopentagend();
            self.state = if self.is_special {
                self.sequence_index = 0;
                State::InSpecialTag
            } else {
                State::Text
            };
            self.section_start = self.index as isize + 1;
        } else if c == b'/' {
            self.state = State::InSelfClosingTag;
        } else if !is_ws(c) {
            self.state = State::InAttributeName;
            self.section_start = self.index as isize;
        }
    }

    fn state_in_self_closing_tag(&mut self, b: &mut Builder, c: u8) {
        if c == b'>' {
            b.onselfclosingtag();
            self.state = State::Text;
            self.section_start = self.index as isize + 1;
            self.is_special = false;
        } else if !is_ws(c) {
            self.state = State::BeforeAttributeName;
            self.state_before_attribute_name(b, c);
        }
    }

    fn state_in_attribute_name(&mut self, b: &mut Builder, c: u8) {
        if c == b'=' || is_end_of_tag_section(c) {
            b.onattribname(self.section());
            self.section_start = -1;
            self.state = State::AfterAttributeName;
            self.state_after_attribute_name(b, c);
        }
    }

    fn state_after_attribute_name(&mut self, b: &mut Builder, c: u8) {
        if c == b'=' {
            self.state = State::BeforeAttributeValue;
        } else if c == b'/' || c == b'>' {
            b.onattribend();
            self.state = State::BeforeAttributeName;
            self.state_before_attribute_name(b, c);
        } else if !is_ws(c) {
            b.onattribend();
            self.state = State::InAttributeName;
            self.section_start = self.index as isize;
        }
    }

    fn state_before_attribute_value(&mut self, b: &mut Builder, c: u8) {
        if c == b'"' {
            self.state = State::InAttributeValueDq;
            self.section_start = self.index as isize + 1;
        } else if c == b'\'' {
            self.state = State::InAttributeValueSq;
            self.section_start = self.index as isize + 1;
        } else if !is_ws(c) {
            self.section_start = self.index as isize;
            self.state = State::InAttributeValueNq;
            self.state_in_attribute_value_nq(b, c);
        }
    }

    fn in_attribute_value(&mut self, b: &mut Builder, c: u8, quote: u8) {
        if c == quote || self.fast_forward_to(quote) {
            b.onattribdata(self.section());
            self.section_start = -1;
            b.onattribend();
            self.state = State::BeforeAttributeName;
        }
    }

    fn state_in_attribute_value_nq(&mut self, b: &mut Builder, c: u8) {
        if is_ws(c) || c == b'>' {
            b.onattribdata(self.section());
            self.section_start = -1;
            b.onattribend();
            self.state = State::BeforeAttributeName;
            self.state_before_attribute_name(b, c);
        }
    }

    fn state_before_declaration(&mut self, c: u8) {
        if c == b'[' {
            self.state = State::CdataSequence;
            self.sequence_index = 0;
        } else {
            self.state = if c == b'-' {
                State::BeforeComment
            } else {
                State::InDeclaration
            };
        }
    }

    fn state_in_declaration(&mut self, b: &mut Builder, c: u8) {
        if c == b'>' || self.fast_forward_to(b'>') {
            b.ondeclaration(self.section());
            self.state = State::Text;
            self.section_start = self.index as isize + 1;
        }
    }

    fn state_in_processing_instruction(&mut self, b: &mut Builder, c: u8) {
        if c == b'>' || self.fast_forward_to(b'>') {
            b.onprocessinginstruction(self.section());
            self.state = State::Text;
            self.section_start = self.index as isize + 1;
        }
    }

    fn state_before_comment(&mut self, c: u8) {
        if c == b'-' {
            self.state = State::InCommentLike;
            self.sequence = COMMENT_END;
            self.sequence_index = 2;
            self.section_start = self.index as isize + 1;
        } else {
            self.state = State::InDeclaration;
        }
    }

    fn state_in_special_comment(&mut self, b: &mut Builder, c: u8) {
        if c == b'>' || self.fast_forward_to(b'>') {
            b.cb_comment(self.section());
            self.state = State::Text;
            self.section_start = self.index as isize + 1;
        }
    }

    fn state_before_special_s(&mut self, b: &mut Builder, c: u8) {
        let lower = c | 0x20;
        if lower == SCRIPT_END[3] {
            self.start_special(SCRIPT_END, 4);
        } else if lower == STYLE_END[3] {
            self.start_special(STYLE_END, 4);
        } else {
            self.state = State::InTagName;
            self.state_in_tag_name(b, c);
        }
    }

    fn run(&mut self, b: &mut Builder) {
        while self.index < self.buf.len() {
            let c = self.buf[self.index];
            match self.state {
                State::Text => self.state_text(b, c),
                State::SpecialStartSequence => self.state_special_start_sequence(b, c),
                State::InSpecialTag => self.state_in_special_tag(b, c),
                State::CdataSequence => self.state_cdata_sequence(b, c),
                State::InAttributeValueDq => self.in_attribute_value(b, c, b'"'),
                State::InAttributeName => self.state_in_attribute_name(b, c),
                State::InCommentLike => self.state_in_comment_like(b, c),
                State::InSpecialComment => self.state_in_special_comment(b, c),
                State::BeforeAttributeName => self.state_before_attribute_name(b, c),
                State::InTagName => self.state_in_tag_name(b, c),
                State::InClosingTagName => self.state_in_closing_tag_name(b, c),
                State::BeforeTagName => self.state_before_tag_name(b, c),
                State::AfterAttributeName => self.state_after_attribute_name(b, c),
                State::InAttributeValueSq => self.in_attribute_value(b, c, b'\''),
                State::BeforeAttributeValue => self.state_before_attribute_value(b, c),
                State::BeforeClosingTagName => self.state_before_closing_tag_name(c),
                State::AfterClosingTagName => self.state_after_closing_tag_name(c),
                State::BeforeSpecialS => self.state_before_special_s(b, c),
                State::InAttributeValueNq => self.state_in_attribute_value_nq(b, c),
                State::InSelfClosingTag => self.state_in_self_closing_tag(b, c),
                State::InDeclaration => self.state_in_declaration(b, c),
                State::BeforeDeclaration => self.state_before_declaration(c),
                State::BeforeComment => self.state_before_comment(c),
                State::InProcessingInstruction => self.state_in_processing_instruction(b, c),
            }
            self.index += 1;
        }
        // `cleanup`: text that is still open is emitted.
        if self.section_start != self.index as isize
            && (self.state == State::Text
                || (self.state == State::InSpecialTag && self.sequence_index == 0))
        {
            let start = self.section_start.max(0) as usize;
            b.cb_text(self.slice(start, self.buf.len()));
            self.section_start = self.index as isize;
        }
        // `finish`, then `handleTrailingData`.
        if self.section_start < self.index as isize {
            // JavaScript cut the buffer at `index` when the section had
            // ended (`-1`), so nothing is left.
            let data = match self.section_start {
                s if s < 0 => "",
                s => self.slice(s as usize, self.buf.len()),
            };
            match self.state {
                State::InCommentLike => {
                    if self.sequence == CDATA_END {
                        b.oncdata(data);
                    } else {
                        b.cb_comment(data);
                    }
                }
                State::InTagName
                | State::BeforeAttributeName
                | State::BeforeAttributeValue
                | State::AfterAttributeName
                | State::InAttributeName
                | State::InAttributeValueSq
                | State::InAttributeValueDq
                | State::InAttributeValueNq
                | State::InClosingTagName => {}
                _ => b.cb_text(data),
            }
        }
        b.onend();
    }
}

/// `posthtml-parser`'s tree for `html`.
pub fn parse(html: &str) -> Vec<Node> {
    let mut b = Builder::new();
    let mut t = Tokenizer {
        buf: html.as_bytes(),
        text: html,
        state: State::Text,
        section_start: 0,
        index: 0,
        is_special: false,
        sequence: COMMENT_END,
        sequence_index: 0,
    };
    t.run(&mut b);
    b.root
}

/// `posthtml-render`'s `is-json` 2.0.1 check: a value that looks like JSON
/// is printed in single quotes, as written.
fn is_json(s: &str) -> bool {
    let s: String = s.chars().filter(|&c| !js_space(c)).collect();
    looks_json(&s)
}

fn looks_json(s: &str) -> bool {
    if s.len() >= 2 && s.starts_with('{') && s.ends_with('}') {
        // `/"(.*?)":(.*?)/`: a `"`, then `":` somewhere after it.
        return match s.find('"') {
            Some(i) => s[i + 1..].contains("\":"),
            None => false,
        };
    }
    if s.len() >= 2 && s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        let joined = inner.replace("},{", "}\n{");
        // `reduce` without a start value gives the last item's result.
        return joined.split('\n').map(looks_json).next_back().unwrap_or(false);
    }
    false
}

/// Print a tree as `posthtml-render` does.
pub fn render(nodes: &[Node], closing: Closing) -> String {
    let mut out = String::new();
    render_into(&mut out, nodes, closing);
    out
}

fn render_into(out: &mut String, nodes: &[Node], closing: Closing) {
    for node in nodes {
        match node {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) => {
                out.push('<');
                out.push_str(&e.tag);
                if let Some(attrs) = &e.attrs {
                    for (k, v) in attrs {
                        if is_json(v) {
                            out.push_str(&format!(" {k}='{v}'"));
                        } else {
                            out.push_str(&format!(" {k}=\"{}\"", v.replace('"', "&quot;")));
                        }
                    }
                }
                if SINGLE.contains(&e.tag.to_lowercase().as_str()) {
                    out.push_str(match closing {
                        Closing::Default => ">",
                        Closing::Slash => " />",
                    });
                    render_into(out, &e.content, closing);
                } else {
                    out.push('>');
                    render_into(out, &e.content, closing);
                    out.push_str("</");
                    out.push_str(&e.tag);
                    out.push('>');
                }
            }
        }
    }
}

/// Every element of the tree, depth first, as `tree.walk` visits them.
pub fn walk_mut(nodes: &mut [Node], f: &mut dyn FnMut(&mut Element)) {
    for node in nodes {
        if let Node::Element(e) = node {
            f(e);
            walk_mut(&mut e.content, f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(html: &str) -> String {
        render(&parse(html), Closing::Default)
    }

    /// Probed: `posthtml().process(html)` with posthtml 0.16.7.
    #[test]
    fn round_trips_like_posthtml() {
        for (input, want) in [
            ("<p>a<b>c</b></p>", "<p>a<b>c</b></p>"),
            ("<input disabled>", "<input disabled=\"\">"),
            ("<img src='a\"b'/>", "<img src=\"a&quot;b\">"),
            ("<p>a<div>b</div>", "<p>a</p><div>b</div>"),
            ("</p>", "<p></p>"),
            ("</br>", "<br>"),
            ("<IMG src=x>after", "<IMG src=\"x\">after"),
            ("<?xml version=\"1.0\"?><a>", "<a></a>"),
            ("<!DOCTYPE html><p>", "<!DOCTYPE html><p></p>"),
            ("<![CDATA[x]]>", "<!--[CDATA[x]]-->"),
            ("<div data-x='{\"a\":1}'>", "<div data-x='{\"a\":1}'></div>"),
            ("<a b=1 b=2>", "<a b=\"1\"></a>"),
            ("<a z=1 2=x 1=y>", "<a 1=\"y\" 2=\"x\" z=\"1\"></a>"),
            (
                "<script>if (a<b) x('</p>')</script>",
                "<script>if (a<b) x('</p>')</script>",
            ),
            ("text <div", "text "),
            ("a < b", "a < b"),
            ("</>x", "</>x"),
            ("<!-- c -->", "<!-- c -->"),
        ] {
            assert_eq!(round(input), want, "{input}");
        }
    }

    #[test]
    fn json_attributes() {
        assert!(is_json("{\"a\": 1}"));
        assert!(!is_json("{a: 1}"));
        assert!(is_json("[{\"a\":1},{\"b\":2}]"));
        assert!(!is_json("[1,2]"));
        assert!(!is_json("1"));
    }
}
