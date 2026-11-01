//! Bounded HTML5 parser experiment with a project-owned, index-based DOM tree.
//!
//! `html5ever` supplies HTML tokenization and tree-construction algorithms. This
//! crate owns the resulting node representation and enforces input, node and
//! parse-error budgets. It can export a bounded versioned flat snapshot for a
//! future process boundary; live DOM bindings, CSS, layout and Network Service
//! integration remain outside this crate.

use std::borrow::Cow;
use std::cell::{Cell, Ref, RefCell};

use html5ever::interface::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, QualName, parse_document};

pub const MAX_HTML_INPUT_BYTES: usize = 1024 * 1024;
pub const MAX_DOM_NODES: usize = 65_536;
pub const MAX_PARSE_ERRORS: usize = 256;
pub const MAX_DOM_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const DOCUMENT_NODE: usize = 0;
const BUDGET_SENTINEL_NODE: usize = 1;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NodeId(usize);

impl NodeId {
    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HtmlNodeData {
    Document,
    DocumentFragment,
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },
    Element {
        name: QualName,
        attributes: Vec<Attribute>,
        template_contents: Option<NodeId>,
        mathml_annotation_xml_integration_point: bool,
        script_already_started: bool,
    },
    Text(String),
    Comment(String),
    ProcessingInstruction {
        target: String,
        contents: String,
    },
    BudgetSentinel {
        name: QualName,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlNode {
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    data: HtmlNodeData,
}

impl HtmlNode {
    pub const fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    pub fn children(&self) -> &[NodeId] {
        &self.children
    }

    pub fn data(&self) -> &HtmlNodeData {
        &self.data
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtmlParseLimits {
    pub max_input_bytes: usize,
    pub max_nodes: usize,
    pub max_parse_errors: usize,
}

impl Default for HtmlParseLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: MAX_HTML_INPUT_BYTES,
            max_nodes: MAX_DOM_NODES,
            max_parse_errors: MAX_PARSE_ERRORS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HtmlParseError {
    InvalidLimits,
    InputTooLarge { actual: usize, limit: usize },
    NodeLimitExceeded { limit: usize },
}

impl std::fmt::Display for HtmlParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimits => formatter.write_str("HTML parser limits are invalid"),
            Self::InputTooLarge { actual, limit } => {
                write!(formatter, "HTML input is {actual} bytes; limit is {limit}")
            }
            Self::NodeLimitExceeded { limit } => {
                write!(formatter, "HTML DOM node count exceeded limit {limit}")
            }
        }
    }
}

impl std::error::Error for HtmlParseError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HtmlSnapshotError {
    Serialization(String),
    TooLarge { actual: usize, limit: usize },
}

impl std::fmt::Display for HtmlSnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialization(message) => {
                write!(formatter, "DOM snapshot serialization failed: {message}")
            }
            Self::TooLarge { actual, limit } => write!(
                formatter,
                "DOM snapshot is {actual} bytes; limit is {limit}"
            ),
        }
    }
}

impl std::error::Error for HtmlSnapshotError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlDocument {
    nodes: Vec<HtmlNode>,
    quirks_mode: QuirksMode,
    parse_errors: Vec<String>,
    suppressed_parse_errors: usize,
    node_limit_exceeded: bool,
}

impl HtmlDocument {
    pub fn node(&self, id: NodeId) -> Option<&HtmlNode> {
        self.nodes.get(id.0)
    }

    pub fn root(&self) -> &HtmlNode {
        &self.nodes[DOCUMENT_NODE]
    }

    pub const fn quirks_mode(&self) -> QuirksMode {
        self.quirks_mode
    }

    pub fn parse_errors(&self) -> &[String] {
        &self.parse_errors
    }

    pub const fn suppressed_parse_errors(&self) -> usize {
        self.suppressed_parse_errors
    }

    pub fn node_count(&self) -> usize {
        // The budget sentinel is an internal parser guard, not part of the DOM.
        self.nodes.len().saturating_sub(1)
    }

    pub fn document_element(&self) -> Option<NodeId> {
        self.root().children.iter().copied().find(|id| {
            matches!(
                &self.nodes[id.0].data,
                HtmlNodeData::Element { name, .. }
                    if name.ns == html5ever::ns!(html) && name.local == html5ever::local_name!(html)
            )
        })
    }

    pub fn find_first_element_by_tag_name(&self, tag_name: &str) -> Option<NodeId> {
        self.find_elements_by_tag_name(tag_name).into_iter().next()
    }

    pub fn find_elements_by_tag_name(&self, tag_name: &str) -> Vec<NodeId> {
        let mut matches = Vec::new();
        let mut pending = vec![NodeId(DOCUMENT_NODE)];
        while let Some(id) = pending.pop() {
            let node = &self.nodes[id.0];
            if let HtmlNodeData::Element { name, .. } = &node.data
                && name.ns == html5ever::ns!(html)
                && name.local.to_string().eq_ignore_ascii_case(tag_name)
            {
                matches.push(id);
            }
            pending.extend(node.children.iter().rev().copied());
        }
        matches
    }

    /// Finds elements by local name across HTML, SVG and MathML namespaces.
    pub fn find_elements_by_local_name(&self, local_name: &str) -> Vec<NodeId> {
        let mut matches = Vec::new();
        let mut pending = vec![NodeId(DOCUMENT_NODE)];
        while let Some(id) = pending.pop() {
            let node = &self.nodes[id.0];
            if let HtmlNodeData::Element { name, .. } = &node.data
                && name.local.to_string() == local_name
            {
                matches.push(id);
            }
            pending.extend(node.children.iter().rev().copied());
        }
        matches
    }

    pub fn template_contents(&self, template: NodeId) -> Option<NodeId> {
        match &self.nodes.get(template.0)?.data {
            HtmlNodeData::Element {
                template_contents, ..
            } => *template_contents,
            _ => None,
        }
    }

    pub fn find_element_by_id(&self, id_value: &str) -> Option<NodeId> {
        let mut pending = vec![NodeId(DOCUMENT_NODE)];
        while let Some(id) = pending.pop() {
            let node = &self.nodes[id.0];
            if let HtmlNodeData::Element { attributes, .. } = &node.data
                && attributes.iter().any(|attribute| {
                    attribute.name.ns.is_empty()
                        && attribute.name.local == html5ever::local_name!(id)
                        && attribute.value.as_ref() == id_value
                })
            {
                return Some(id);
            }
            pending.extend(node.children.iter().rev().copied());
        }
        None
    }

    pub fn attribute(&self, element: NodeId, name: &str) -> Option<&str> {
        let HtmlNodeData::Element { attributes, .. } = &self.nodes.get(element.0)?.data else {
            return None;
        };
        attributes.iter().find_map(|attribute| {
            (attribute.name.ns.is_empty() && attribute.name.local.to_string() == name)
                .then_some(attribute.value.as_ref())
        })
    }

    pub fn text_content(&self, id: NodeId) -> Option<String> {
        self.nodes.get(id.0)?;
        let mut output = String::new();
        let mut pending = vec![id];
        while let Some(current) = pending.pop() {
            let node = &self.nodes[current.0];
            if let HtmlNodeData::Text(text) = &node.data {
                output.push_str(text);
            } else {
                pending.extend(node.children.iter().rev().copied());
            }
        }
        Some(output)
    }

    /// Exports an immutable, versioned, flat DOM snapshot for process boundaries.
    /// Node IDs are stable only within this parsed Document generation; callers
    /// must pair them with a Document ID/revision and must not treat this as live DOM.
    pub fn to_snapshot_json(&self) -> Result<Vec<u8>, HtmlSnapshotError> {
        use serde_json::{Value, json};

        let mut nodes = Vec::with_capacity(self.nodes.len().saturating_sub(1));
        for (index, node) in self.nodes.iter().enumerate() {
            if matches!(&node.data, HtmlNodeData::BudgetSentinel { .. }) {
                continue;
            }
            let id = index;
            let parent = node.parent.map(|value| value.0);
            let children: Vec<usize> = node.children.iter().map(|value| value.0).collect();
            let mut entry = json!({
                "id": id,
                "parent": parent,
                "children": children,
            });
            let data =
                match &node.data {
                    HtmlNodeData::Document => json!({"type": "document", "nodeType": 9}),
                    HtmlNodeData::DocumentFragment => {
                        json!({"type": "document-fragment", "nodeType": 11})
                    }
                    HtmlNodeData::Doctype {
                        name,
                        public_id,
                        system_id,
                    } => json!({
                        "type": "doctype", "nodeType": 10, "name": name,
                        "publicId": public_id, "systemId": system_id
                    }),
                    HtmlNodeData::Element {
                        name,
                        attributes,
                        template_contents,
                        ..
                    } => {
                        let attrs: Vec<Value> = attributes.iter().map(|attribute| json!({
                        "namespace": attribute.name.ns.to_string(),
                        "prefix": attribute.name.prefix.as_ref().map(ToString::to_string),
                        "name": attribute.name.local.to_string(),
                        "value": attribute.value.as_ref(),
                    })).collect();
                        json!({
                            "type": "element", "nodeType": 1,
                            "namespace": name.ns.to_string(),
                            "prefix": name.prefix.as_ref().map(ToString::to_string),
                            "name": name.local.to_string(),
                            "attributes": attrs,
                            "templateContents": template_contents.map(|value| value.0),
                        })
                    }
                    HtmlNodeData::Text(value) => {
                        json!({"type": "text", "nodeType": 3, "value": value})
                    }
                    HtmlNodeData::Comment(value) => {
                        json!({"type": "comment", "nodeType": 8, "value": value})
                    }
                    HtmlNodeData::ProcessingInstruction { target, contents } => json!({
                        "type": "processing-instruction", "nodeType": 7,
                        "target": target, "value": contents
                    }),
                    HtmlNodeData::BudgetSentinel { .. } => continue,
                };
            entry
                .as_object_mut()
                .expect("snapshot node is an object")
                .extend(
                    data.as_object()
                        .expect("snapshot data is an object")
                        .clone(),
                );
            nodes.push(entry);
        }
        let snapshot = json!({
            "schemaVersion": 1,
            "root": DOCUMENT_NODE,
            "nodeCount": self.node_count(),
            "quirksMode": format!("{:?}", self.quirks_mode),
            "parseErrors": &self.parse_errors,
            "suppressedParseErrors": self.suppressed_parse_errors,
            "nodes": nodes,
        });
        let encoded = serde_json::to_vec(&snapshot)
            .map_err(|error| HtmlSnapshotError::Serialization(error.to_string()))?;
        if encoded.len() > MAX_DOM_SNAPSHOT_BYTES {
            return Err(HtmlSnapshotError::TooLarge {
                actual: encoded.len(),
                limit: MAX_DOM_SNAPSHOT_BYTES,
            });
        }
        Ok(encoded)
    }

    pub fn serialize_debug_tree(&self) -> String {
        let mut output = String::new();
        let mut pending = vec![(NodeId(DOCUMENT_NODE), 0usize)];
        while let Some((id, depth)) = pending.pop() {
            let node = &self.nodes[id.0];
            output.push_str(&"  ".repeat(depth));
            match &node.data {
                HtmlNodeData::Document => output.push_str("#document"),
                HtmlNodeData::DocumentFragment => output.push_str("#document-fragment"),
                HtmlNodeData::Doctype { name, .. } => {
                    output.push_str(&format!("<!DOCTYPE {name}>"))
                }
                HtmlNodeData::Element { name, .. } => output.push_str(&format!("<{}>", name.local)),
                HtmlNodeData::Text(text) => output.push_str(&format!("#text {:?}", text)),
                HtmlNodeData::Comment(text) => output.push_str(&format!("<!--{text}-->")),
                HtmlNodeData::ProcessingInstruction { target, .. } => {
                    output.push_str(&format!("<?{target}?>"))
                }
                HtmlNodeData::BudgetSentinel { .. } => output.push_str("#budget-sentinel"),
            }
            output.push('\n');
            pending.extend(node.children.iter().rev().map(|child| (*child, depth + 1)));
        }
        output
    }
}

struct HtmlTreeSink {
    nodes: RefCell<Vec<HtmlNode>>,
    quirks_mode: Cell<QuirksMode>,
    parse_errors: RefCell<Vec<String>>,
    suppressed_parse_errors: Cell<usize>,
    max_nodes: usize,
    max_parse_errors: usize,
    node_limit_exceeded: Cell<bool>,
}

impl HtmlTreeSink {
    fn new(limits: HtmlParseLimits) -> Self {
        let document = HtmlNode {
            parent: None,
            children: Vec::new(),
            data: HtmlNodeData::Document,
        };
        let sentinel = HtmlNode {
            parent: None,
            children: Vec::new(),
            data: HtmlNodeData::BudgetSentinel {
                name: QualName::new(
                    None,
                    html5ever::ns!(html),
                    html5ever::LocalName::from("aibe-budget-sentinel"),
                ),
            },
        };
        Self {
            nodes: RefCell::new(vec![document, sentinel]),
            quirks_mode: Cell::new(QuirksMode::NoQuirks),
            parse_errors: RefCell::new(Vec::new()),
            suppressed_parse_errors: Cell::new(0),
            max_nodes: limits.max_nodes,
            max_parse_errors: limits.max_parse_errors,
            node_limit_exceeded: Cell::new(false),
        }
    }

    fn allocate(&self, node: HtmlNode) -> usize {
        let mut nodes = self.nodes.borrow_mut();
        if nodes.len() > self.max_nodes {
            self.node_limit_exceeded.set(true);
            return BUDGET_SENTINEL_NODE;
        }
        let id = nodes.len();
        nodes.push(node);
        id
    }

    fn detach(&self, target: usize) {
        if target <= BUDGET_SENTINEL_NODE {
            return;
        }
        let mut nodes = self.nodes.borrow_mut();
        let Some(parent) = nodes[target].parent.take() else {
            return;
        };
        nodes[parent.0].children.retain(|child| child.0 != target);
    }

    fn append_handle(&self, parent: usize, child: usize) {
        if parent == BUDGET_SENTINEL_NODE || child <= BUDGET_SENTINEL_NODE || parent == child {
            return;
        }
        self.detach(child);
        let mut nodes = self.nodes.borrow_mut();
        if parent >= nodes.len() || child >= nodes.len() {
            return;
        }
        nodes[child].parent = Some(NodeId(parent));
        nodes[parent].children.push(NodeId(child));
    }

    fn append_text(&self, parent: usize, text: StrTendril) {
        if parent == BUDGET_SENTINEL_NODE || text.is_empty() {
            return;
        }
        let previous = self
            .nodes
            .borrow()
            .get(parent)
            .and_then(|node| node.children.last().copied());
        if let Some(previous) = previous {
            let mut nodes = self.nodes.borrow_mut();
            if let HtmlNodeData::Text(contents) = &mut nodes[previous.0].data {
                contents.push_str(text.as_ref());
                return;
            }
        }
        let child = self.allocate(HtmlNode {
            parent: Some(NodeId(parent)),
            children: Vec::new(),
            data: HtmlNodeData::Text(text.to_string()),
        });
        if child != BUDGET_SENTINEL_NODE {
            self.nodes.borrow_mut()[parent].children.push(NodeId(child));
        }
    }

    fn append_before_handle(&self, sibling: usize, child: usize) {
        if sibling <= BUDGET_SENTINEL_NODE || child <= BUDGET_SENTINEL_NODE || sibling == child {
            return;
        }
        let parent = self.nodes.borrow()[sibling].parent;
        let Some(parent) = parent else {
            return;
        };
        self.detach(child);
        let mut nodes = self.nodes.borrow_mut();
        let Some(index) = nodes[parent.0]
            .children
            .iter()
            .position(|candidate| candidate.0 == sibling)
        else {
            return;
        };
        nodes[child].parent = Some(parent);
        nodes[parent.0].children.insert(index, NodeId(child));
    }

    fn append_before_text(&self, sibling: usize, text: StrTendril) {
        if sibling <= BUDGET_SENTINEL_NODE || text.is_empty() {
            return;
        }
        let parent = self.nodes.borrow()[sibling].parent;
        let Some(parent) = parent else {
            return;
        };
        let previous = {
            let nodes = self.nodes.borrow();
            let children = &nodes[parent.0].children;
            let index = children.iter().position(|child| child.0 == sibling);
            index.and_then(|index| index.checked_sub(1).map(|previous| children[previous]))
        };
        if let Some(previous) = previous {
            let mut nodes = self.nodes.borrow_mut();
            if let HtmlNodeData::Text(contents) = &mut nodes[previous.0].data {
                contents.push_str(text.as_ref());
                return;
            }
        }
        let child = self.allocate(HtmlNode {
            parent: Some(parent),
            children: Vec::new(),
            data: HtmlNodeData::Text(text.to_string()),
        });
        if child != BUDGET_SENTINEL_NODE {
            let mut nodes = self.nodes.borrow_mut();
            if let Some(index) = nodes[parent.0]
                .children
                .iter()
                .position(|candidate| candidate.0 == sibling)
            {
                nodes[parent.0].children.insert(index, NodeId(child));
            }
        }
    }
}

impl TreeSink for HtmlTreeSink {
    type Handle = usize;
    type Output = HtmlDocument;
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Self::Output {
        HtmlDocument {
            nodes: self.nodes.into_inner(),
            quirks_mode: self.quirks_mode.get(),
            parse_errors: self.parse_errors.into_inner(),
            suppressed_parse_errors: self.suppressed_parse_errors.get(),
            node_limit_exceeded: self.node_limit_exceeded.get(),
        }
    }

    fn parse_error(&self, message: Cow<'static, str>) {
        let mut errors = self.parse_errors.borrow_mut();
        if errors.len() < self.max_parse_errors {
            errors.push(message.into_owned());
        } else {
            self.suppressed_parse_errors
                .set(self.suppressed_parse_errors.get().saturating_add(1));
        }
    }

    fn get_document(&self) -> Self::Handle {
        DOCUMENT_NODE
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        Ref::map(self.nodes.borrow(), |nodes| match &nodes[*target].data {
            HtmlNodeData::Element { name, .. } | HtmlNodeData::BudgetSentinel { name } => name,
            _ => panic!("HTML tree builder requested the name of a non-element"),
        })
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> Self::Handle {
        let template_contents = if flags.template {
            Some(self.allocate(HtmlNode {
                parent: None,
                children: Vec::new(),
                data: HtmlNodeData::DocumentFragment,
            }))
        } else {
            None
        };
        if template_contents == Some(BUDGET_SENTINEL_NODE) {
            return BUDGET_SENTINEL_NODE;
        }
        self.allocate(HtmlNode {
            parent: None,
            children: Vec::new(),
            data: HtmlNodeData::Element {
                name,
                attributes: attrs,
                template_contents: template_contents.map(NodeId),
                mathml_annotation_xml_integration_point: flags
                    .mathml_annotation_xml_integration_point,
                script_already_started: false,
            },
        })
    }

    fn create_comment(&self, text: StrTendril) -> Self::Handle {
        self.allocate(HtmlNode {
            parent: None,
            children: Vec::new(),
            data: HtmlNodeData::Comment(text.to_string()),
        })
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Self::Handle {
        self.allocate(HtmlNode {
            parent: None,
            children: Vec::new(),
            data: HtmlNodeData::ProcessingInstruction {
                target: target.to_string(),
                contents: data.to_string(),
            },
        })
    }

    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        match child {
            NodeOrText::AppendNode(child) => self.append_handle(*parent, child),
            NodeOrText::AppendText(text) => self.append_text(*parent, text),
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &Self::Handle,
        previous_element: &Self::Handle,
        child: NodeOrText<Self::Handle>,
    ) {
        let has_parent = self
            .nodes
            .borrow()
            .get(*element)
            .and_then(|node| node.parent)
            .is_some();
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(previous_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        let doctype = self.allocate(HtmlNode {
            parent: Some(NodeId(DOCUMENT_NODE)),
            children: Vec::new(),
            data: HtmlNodeData::Doctype {
                name: name.to_string(),
                public_id: public_id.to_string(),
                system_id: system_id.to_string(),
            },
        });
        if doctype != BUDGET_SENTINEL_NODE {
            self.nodes.borrow_mut()[DOCUMENT_NODE]
                .children
                .push(NodeId(doctype));
        }
    }

    fn mark_script_already_started(&self, node: &Self::Handle) {
        if *node <= BUDGET_SENTINEL_NODE {
            return;
        }
        if let HtmlNodeData::Element {
            script_already_started,
            ..
        } = &mut self.nodes.borrow_mut()[*node].data
        {
            *script_already_started = true;
        }
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        if *target == BUDGET_SENTINEL_NODE {
            return BUDGET_SENTINEL_NODE;
        }
        match &self.nodes.borrow()[*target].data {
            HtmlNodeData::Element {
                template_contents: Some(contents),
                ..
            } => contents.0,
            _ => panic!("HTML tree builder requested missing template contents"),
        }
    }

    fn same_node(&self, left: &Self::Handle, right: &Self::Handle) -> bool {
        left == right
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.quirks_mode.set(mode);
    }

    fn append_before_sibling(&self, sibling: &Self::Handle, child: NodeOrText<Self::Handle>) {
        match child {
            NodeOrText::AppendNode(child) => self.append_before_handle(*sibling, child),
            NodeOrText::AppendText(text) => self.append_before_text(*sibling, text),
        }
    }

    fn add_attrs_if_missing(&self, target: &Self::Handle, attrs: Vec<Attribute>) {
        if *target <= BUDGET_SENTINEL_NODE {
            return;
        }
        if let HtmlNodeData::Element {
            attributes: existing,
            ..
        } = &mut self.nodes.borrow_mut()[*target].data
        {
            for attribute in attrs {
                if !existing
                    .iter()
                    .any(|current| current.name == attribute.name)
                {
                    existing.push(attribute);
                }
            }
        }
    }

    fn remove_from_parent(&self, target: &Self::Handle) {
        self.detach(*target);
    }

    fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
        if *node <= BUDGET_SENTINEL_NODE || *new_parent <= BUDGET_SENTINEL_NODE {
            return;
        }
        let children = std::mem::take(&mut self.nodes.borrow_mut()[*node].children);
        for child in children {
            self.append_handle(*new_parent, child.0);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &Self::Handle) -> bool {
        if *handle <= BUDGET_SENTINEL_NODE {
            return false;
        }
        matches!(
            &self.nodes.borrow()[*handle].data,
            HtmlNodeData::Element {
                mathml_annotation_xml_integration_point: true,
                ..
            }
        )
    }
}

pub fn parse_html(input: &[u8]) -> Result<HtmlDocument, HtmlParseError> {
    parse_html_with_limits(input, HtmlParseLimits::default())
}

pub fn parse_html_with_limits(
    input: &[u8],
    limits: HtmlParseLimits,
) -> Result<HtmlDocument, HtmlParseError> {
    if limits.max_input_bytes == 0
        || limits.max_input_bytes > MAX_HTML_INPUT_BYTES
        || limits.max_nodes < 2
        || limits.max_nodes > MAX_DOM_NODES
        || limits.max_parse_errors > MAX_PARSE_ERRORS
    {
        return Err(HtmlParseError::InvalidLimits);
    }
    if input.len() > limits.max_input_bytes {
        return Err(HtmlParseError::InputTooLarge {
            actual: input.len(),
            limit: limits.max_input_bytes,
        });
    }

    let sink = HtmlTreeSink::new(limits);
    let document = parse_document(sink, Default::default())
        .from_utf8()
        .one(input);
    if document.node_limit_exceeded {
        return Err(HtmlParseError::NodeLimitExceeded {
            limit: limits.max_nodes,
        });
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::{
        HtmlNodeData, HtmlParseError, HtmlParseLimits, MAX_DOM_NODES, MAX_HTML_INPUT_BYTES,
        parse_html, parse_html_with_limits,
    };

    #[test]
    fn exports_versioned_flat_snapshot_with_stable_document_local_ids() {
        let document = parse_html(
            "<!doctype html><p id='greeting' title='A &amp; B'>Hello 世界</p><svg><circle/></svg>"
                .as_bytes(),
        )
        .unwrap();
        let encoded = document.to_snapshot_json().unwrap();
        let snapshot: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(snapshot["schemaVersion"], 1);
        assert_eq!(snapshot["root"], 0);
        assert_eq!(
            snapshot["nodeCount"].as_u64().unwrap() as usize,
            document.node_count()
        );
        let nodes = snapshot["nodes"].as_array().unwrap();
        assert_eq!(nodes[0]["type"], "document");
        let paragraph = nodes
            .iter()
            .find(|node| node["type"] == "element" && node["name"] == "p")
            .unwrap();
        let paragraph_id = paragraph["id"].as_u64().unwrap();
        assert!(nodes.iter().any(|node| node["parent"] == paragraph_id
            && node["type"] == "text"
            && node["value"] == "Hello 世界"));
        assert!(
            paragraph["attributes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|attribute| attribute["name"] == "title" && attribute["value"] == "A & B")
        );
        let svg = nodes.iter().find(|node| node["name"] == "svg").unwrap();
        assert_eq!(svg["namespace"], "http://www.w3.org/2000/svg");
        assert!(nodes.iter().any(|node| node["type"] == "doctype"));
        assert!(std::str::from_utf8(&encoded).is_ok());
    }

    #[test]
    fn parses_document_structure_text_and_named_character_references() {
        let document = parse_html(
            b"<!doctype html><title>Example</title><p id='intro'>Hello &amp; <b>world</b>!</p>",
        )
        .unwrap();
        let html = document.document_element().expect("implied html element");
        let title = document.find_first_element_by_tag_name("title").unwrap();
        let paragraph = document.find_element_by_id("intro").unwrap();
        assert_eq!(
            document.quirks_mode(),
            html5ever::interface::tree_builder::QuirksMode::NoQuirks
        );
        assert_eq!(document.attribute(paragraph, "id"), Some("intro"));
        assert_eq!(document.text_content(title).as_deref(), Some("Example"));
        assert_eq!(
            document.text_content(paragraph).as_deref(),
            Some("Hello & world!")
        );
        assert!(document.node(html).is_some());
        assert!(document.parse_errors().is_empty());
    }

    #[test]
    fn html5_tree_builder_repairs_malformed_paragraph_nesting() {
        let document = parse_html(b"<p>first<p>second").unwrap();
        let paragraphs = document.find_elements_by_tag_name("p");
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(
            document.text_content(paragraphs[0]).as_deref(),
            Some("first")
        );
        assert_eq!(
            document.text_content(paragraphs[1]).as_deref(),
            Some("second")
        );
        assert!(!document.parse_errors().is_empty());
    }

    #[test]
    fn raw_text_elements_do_not_parse_markup_or_character_references() {
        let document = parse_html(
            b"<script>if (a < b) value = '&amp;';</script><style>.x::before { content: '<tag>'; }</style>",
        )
        .unwrap();
        let script = document.find_first_element_by_tag_name("script").unwrap();
        let style = document.find_first_element_by_tag_name("style").unwrap();
        assert_eq!(
            document.text_content(script).as_deref(),
            Some("if (a < b) value = '&amp;';")
        );
        assert_eq!(
            document.text_content(style).as_deref(),
            Some(".x::before { content: '<tag>'; }")
        );
    }

    #[test]
    fn preserves_svg_mathml_namespaces_and_template_contents() {
        let document = parse_html(
            b"<svg><circle cx='4'></circle></svg><math><mi>x</mi></math><template><p>hidden</p></template>",
        )
        .unwrap();
        let svg = document.find_elements_by_local_name("svg")[0];
        let circle = document.find_elements_by_local_name("circle")[0];
        let math = document.find_elements_by_local_name("math")[0];
        let mi = document.find_elements_by_local_name("mi")[0];
        let template = document.find_first_element_by_tag_name("template").unwrap();
        let svg_name = match document.node(svg).unwrap().data() {
            HtmlNodeData::Element { name, .. } => name,
            _ => panic!("expected SVG element"),
        };
        let math_name = match document.node(math).unwrap().data() {
            HtmlNodeData::Element { name, .. } => name,
            _ => panic!("expected MathML element"),
        };
        assert_eq!(svg_name.ns, html5ever::ns!(svg));
        assert_eq!(math_name.ns, html5ever::ns!(mathml));
        assert_eq!(
            match document.node(circle).unwrap().data() {
                HtmlNodeData::Element { name, .. } => name.ns.clone(),
                _ => panic!("expected circle element"),
            },
            html5ever::ns!(svg)
        );
        assert_eq!(
            match document.node(mi).unwrap().data() {
                HtmlNodeData::Element { name, .. } => name.ns.clone(),
                _ => panic!("expected mi element"),
            },
            html5ever::ns!(mathml)
        );
        let template_contents = document.template_contents(template).unwrap();
        assert_eq!(document.text_content(template).as_deref(), Some(""));
        assert_eq!(
            document.text_content(template_contents).as_deref(),
            Some("hidden")
        );
    }

    #[test]
    fn duplicate_attributes_keep_the_first_value() {
        let document = parse_html(b"<div id='first' id='second' class='x'></div>").unwrap();
        let div = document.find_first_element_by_tag_name("div").unwrap();
        assert_eq!(document.attribute(div, "id"), Some("first"));
        assert_eq!(document.attribute(div, "class"), Some("x"));
    }

    #[test]
    fn rejects_input_larger_than_the_configured_hard_limit() {
        let input = vec![b'a'; MAX_HTML_INPUT_BYTES + 1];
        assert!(matches!(
            parse_html(&input),
            Err(HtmlParseError::InputTooLarge {
                actual,
                limit: MAX_HTML_INPUT_BYTES
            }) if actual == MAX_HTML_INPUT_BYTES + 1
        ));
    }

    #[test]
    fn rejects_dom_node_budget_exhaustion_without_allocating_unbounded_nodes() {
        let limits = HtmlParseLimits {
            max_input_bytes: 1024,
            max_nodes: 8,
            max_parse_errors: 8,
        };
        assert!(matches!(
            parse_html_with_limits(b"<div><p>one</p><p>two</p><p>three</p></div>", limits),
            Err(HtmlParseError::NodeLimitExceeded { limit: 8 })
        ));
        assert_eq!(MAX_DOM_NODES, 65_536);
    }

    #[test]
    fn caps_recorded_parse_errors_and_counts_suppressed_errors() {
        let limits = HtmlParseLimits {
            max_input_bytes: 1024,
            max_nodes: 128,
            max_parse_errors: 1,
        };
        let document = parse_html_with_limits(b"<p><div><p><table><tr><td>x", limits).unwrap();
        assert!(document.parse_errors().len() <= 1);
        assert!(document.suppressed_parse_errors() > 0);
    }

    #[test]
    fn rejects_invalid_parser_limits() {
        let limits = HtmlParseLimits {
            max_input_bytes: MAX_HTML_INPUT_BYTES + 1,
            ..HtmlParseLimits::default()
        };
        assert_eq!(
            parse_html_with_limits(b"<p>x</p>", limits),
            Err(HtmlParseError::InvalidLimits)
        );
    }
}
