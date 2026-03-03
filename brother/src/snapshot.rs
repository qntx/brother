//! Accessibility snapshot with ref-based element targeting.
//!
//! The snapshot system captures the browser's accessibility tree and assigns
//! stable refs (`e1`, `e2`, ...) to interactive elements. These refs can then
//! be used to click, fill, or read elements without CSS selectors.

use std::collections::HashMap;
use std::fmt;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// A captured accessibility snapshot with element refs.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The formatted accessibility tree string.
    tree: String,
    /// Map from ref id (e.g. `"e1"`) to element metadata.
    refs: RefMap,
}

impl Snapshot {
    /// Create a new snapshot from a tree string and ref map.
    pub(crate) const fn new(tree: String, refs: RefMap) -> Self {
        Self { tree, refs }
    }

    /// The formatted accessibility tree as a string.
    ///
    /// This is the primary output for AI agents to parse.
    #[must_use]
    pub fn tree(&self) -> &str {
        &self.tree
    }

    /// The ref map for element lookup.
    #[must_use]
    pub const fn refs(&self) -> &RefMap {
        &self.refs
    }

    /// Look up a ref by id (e.g. `"e1"`).
    #[must_use]
    pub fn get_ref(&self, id: &str) -> Option<&Ref> {
        self.refs.get(id)
    }

    /// Total number of refs in this snapshot.
    #[must_use]
    pub fn ref_count(&self) -> usize {
        self.refs.len()
    }
}

impl Snapshot {
    /// Append cursor-interactive elements detected via JS to the snapshot.
    ///
    /// These are elements with `cursor:pointer`, `onclick`, or `tabindex` that
    /// lack proper ARIA roles and were not captured by the accessibility tree.
    pub fn append_cursor_elements(&mut self, items: &[CursorItem]) {
        if items.is_empty() {
            return;
        }

        // Dedup: skip items whose text already appears in existing ref names
        let existing: std::collections::HashSet<String> = self
            .refs
            .values()
            .map(|r| r.name.to_ascii_lowercase())
            .collect();

        let next_id = self
            .refs
            .keys()
            .filter_map(|k| k.strip_prefix('e')?.parse::<u32>().ok())
            .max()
            .unwrap_or(0);

        let mut extra_lines = Vec::new();
        let mut counter = next_id;

        for item in items {
            if existing.contains(&item.text.to_ascii_lowercase()) {
                continue;
            }
            counter += 1;
            let id = format!("e{counter}");
            let role = "clickable";

            self.refs.insert(
                id.clone(),
                Ref {
                    role: role.to_owned(),
                    name: item.text.clone(),
                    backend_node_id: 0,
                    nth: None,
                    focusable: true,
                },
            );

            extra_lines.push(format!(
                "- {role} \"{}\" [ref={id}] [{}]",
                item.text, item.hints
            ));
        }

        if !extra_lines.is_empty() {
            self.tree.push_str("\n# Cursor-interactive elements:\n");
            self.tree.push_str(&extra_lines.join("\n"));
        }
    }
}

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.tree)
    }
}

/// Map from ref id to element metadata.
pub type RefMap = HashMap<String, Ref>;

/// Metadata for a single referenced element in the accessibility tree.
///
/// Refs are resolved via ARIA role + accessible name (re-queried from live DOM),
/// so they survive DOM mutations as long as the element is still present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ref {
    /// ARIA role (e.g. `"button"`, `"link"`, `"textbox"`).
    pub role: String,
    /// Accessible name (e.g. button label, link text).
    pub name: String,
    /// CDP `backendDOMNodeId` — used as a fast-path when the DOM hasn't changed.
    /// Falls back to role+name resolution if this id becomes stale.
    pub backend_node_id: i64,
    /// Index for disambiguation when multiple elements share the same role+name.
    /// `None` means this role+name combination is unique on the page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nth: Option<usize>,
    /// Whether this element is focusable.
    pub focusable: bool,
}

/// A cursor-interactive element detected via JS (not in the AX tree).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorItem {
    /// Visible text content (trimmed, max 100 chars).
    pub text: String,
    /// Why it's interactive (e.g. "cursor:pointer, onclick").
    pub hints: String,
}

/// Options for filtering and formatting a snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnapshotOptions {
    /// Only include interactive elements (buttons, links, inputs, etc.).
    pub interactive_only: bool,
    /// Remove empty structural elements.
    pub compact: bool,
    /// Maximum tree depth (0 = unlimited).
    pub max_depth: usize,
    /// CSS selector to scope the snapshot subtree.
    pub selector: Option<String>,
    /// Also detect cursor-interactive elements (`cursor:pointer`, `onclick`,
    /// `tabindex`) that lack proper ARIA roles and append them to the tree.
    pub cursor_interactive: bool,
}

impl SnapshotOptions {
    /// Only include interactive elements.
    #[must_use]
    pub const fn interactive_only(mut self, v: bool) -> Self {
        self.interactive_only = v;
        self
    }

    /// Remove empty structural elements.
    #[must_use]
    pub const fn compact(mut self, v: bool) -> Self {
        self.compact = v;
        self
    }

    /// Limit tree depth.
    #[must_use]
    pub const fn max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    /// Scope snapshot to a CSS selector subtree.
    #[must_use]
    pub fn selector(mut self, sel: impl Into<String>) -> Self {
        self.selector = Some(sel.into());
        self
    }

    /// Include cursor-interactive elements (cursor:pointer, onclick, tabindex).
    #[must_use]
    pub const fn cursor_interactive(mut self, v: bool) -> Self {
        self.cursor_interactive = v;
        self
    }
}

/// Roles considered interactive — these always get refs.
pub const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "link",
    "textbox",
    "checkbox",
    "radio",
    "combobox",
    "listbox",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "searchbox",
    "slider",
    "spinbutton",
    "switch",
    "tab",
    "treeitem",
];

/// Roles that carry meaningful content — get refs for text extraction.
pub const CONTENT_ROLES: &[&str] = &[
    "heading",
    "cell",
    "gridcell",
    "columnheader",
    "rowheader",
    "listitem",
    "article",
    "definition",
    "figure",
    "img",
    "math",
    "meter",
    "progressbar",
    "status",
    "tooltip",
];

/// Structural roles that can be pruned in compact mode when empty.
pub const STRUCTURAL_ROLES: &[&str] = &[
    "generic",
    "group",
    "list",
    "navigation",
    "region",
    "section",
    "banner",
    "complementary",
    "contentinfo",
    "main",
    "form",
];

/// Check if a role is interactive.
#[must_use]
pub fn is_interactive(role: &str) -> bool {
    INTERACTIVE_ROLES
        .iter()
        .any(|&r| r.eq_ignore_ascii_case(role))
}

/// Check if a role carries content.
#[must_use]
pub fn is_content(role: &str) -> bool {
    CONTENT_ROLES.iter().any(|&r| r.eq_ignore_ascii_case(role))
}

/// Check if a role is purely structural.
#[must_use]
pub fn is_structural(role: &str) -> bool {
    STRUCTURAL_ROLES
        .iter()
        .any(|&r| r.eq_ignore_ascii_case(role))
}

/// Mutable state accumulated during snapshot rendering.
struct RenderCtx {
    ref_counter: u32,
    refs: RefMap,
    lines: Vec<String>,
}

/// Build a [`Snapshot`] from the raw CDP accessibility tree nodes.
///
/// `nodes` should come from `Accessibility.getFullAXTree` CDP response.
pub fn build_snapshot(nodes: &[serde_json::Value], options: &SnapshotOptions) -> Snapshot {
    let mut ctx = RenderCtx {
        ref_counter: 0,
        refs: RefMap::new(),
        lines: Vec::new(),
    };

    // CDP returns a flat list; the first node is the root.
    // Each node has `childIds` pointing to children by `nodeId`.
    let node_map: HashMap<String, &serde_json::Value> = nodes
        .iter()
        .filter_map(|n| {
            n.get("nodeId")
                .and_then(serde_json::Value::as_str)
                .map(|id| (id.to_owned(), n))
        })
        .collect();

    if let Some(root) = nodes
        .first()
        .and_then(|n| n.get("nodeId").and_then(serde_json::Value::as_str))
    {
        render_node(root, &node_map, options, 0, &mut ctx);
    }

    // Post-pass: disambiguate refs that share the same (role, name).
    // Count occurrences of each (role, name) pair, then assign nth indices
    // only when there are duplicates (nth stays None for unique elements).
    disambiguate_refs(&mut ctx.refs);

    Snapshot::new(ctx.lines.join("\n"), ctx.refs)
}

/// Recursively render a single AX node into the output lines.
fn render_node(
    node_id: &str,
    node_map: &HashMap<String, &serde_json::Value>,
    options: &SnapshotOptions,
    depth: usize,
    ctx: &mut RenderCtx,
) {
    let Some(node) = node_map.get(node_id) else {
        return;
    };

    let role = ax_str(node, "role");
    let name = ax_str(node, "name");

    // Depth limit
    if options.max_depth > 0 && depth > options.max_depth {
        return;
    }

    // Skip ignored nodes
    if node
        .get("ignored")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        // Still recurse into children — ignored containers may have visible children
        render_children(node, node_map, options, depth, ctx);
        return;
    }

    let interactive = is_interactive(&role);
    let content = is_content(&role);
    let structural = is_structural(&role);

    // Interactive-only filter
    if options.interactive_only && !interactive {
        // Still recurse — interactive elements may be nested inside non-interactive containers
        render_children(node, node_map, options, depth, ctx);
        return;
    }

    // Compact mode: skip empty structural nodes
    if options.compact && structural && name.is_empty() {
        render_children(node, node_map, options, depth, ctx);
        return;
    }

    // Assign ref if interactive or content-bearing
    let ref_id = if interactive || content {
        ctx.ref_counter += 1;
        let id = format!("e{}", ctx.ref_counter);

        let backend_node_id = node
            .get("backendDOMNodeId")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);

        let focusable = node
            .get("properties")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|props| {
                props.iter().any(|p| {
                    p.get("name").and_then(serde_json::Value::as_str) == Some("focusable")
                        && p.get("value")
                            .and_then(|v| v.get("value"))
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                })
            });

        ctx.refs.insert(
            id.clone(),
            Ref {
                role: role.clone(),
                name: name.clone(),
                backend_node_id,
                nth: None,
                focusable,
            },
        );
        Some(id)
    } else {
        None
    };

    // Format line
    let indent = "  ".repeat(depth);
    let mut line = format!("{indent}- {role}");

    if !name.is_empty() {
        let _ = write!(line, " \"{name}\"");
    }

    if let Some(ref id) = ref_id {
        let _ = write!(line, " [ref={id}]");
    }

    // Append extra properties (level for headings, checked state, etc.)
    append_properties(node, &role, &mut line);

    ctx.lines.push(line);

    // Recurse into children
    render_children(node, node_map, options, depth + 1, ctx);
}

/// Render all children of a node.
fn render_children(
    node: &serde_json::Value,
    node_map: &HashMap<String, &serde_json::Value>,
    options: &SnapshotOptions,
    depth: usize,
    ctx: &mut RenderCtx,
) {
    let Some(children) = node.get("childIds").and_then(serde_json::Value::as_array) else {
        return;
    };
    for child_id in children {
        if let Some(id) = child_id.as_str() {
            render_node(id, node_map, options, depth, ctx);
        }
    }
}

/// Append relevant ARIA properties to the output line.
fn append_properties(node: &serde_json::Value, role: &str, line: &mut String) {
    let Some(props) = node.get("properties").and_then(serde_json::Value::as_array) else {
        return;
    };

    for prop in props {
        let prop_name = prop
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let prop_value = prop.get("value").and_then(|v| v.get("value"));

        match prop_name {
            "level" if role == "heading" => {
                if let Some(level) = prop_value.and_then(serde_json::Value::as_u64) {
                    let _ = write!(line, " [level={level}]");
                }
            }
            "checked" => {
                if let Some(checked) = prop_value.and_then(serde_json::Value::as_str) {
                    let _ = write!(line, " [checked={checked}]");
                }
            }
            "disabled" => {
                if prop_value
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    line.push_str(" [disabled]");
                }
            }
            "required" => {
                if prop_value
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    line.push_str(" [required]");
                }
            }
            "expanded" => {
                if let Some(expanded) = prop_value.and_then(serde_json::Value::as_bool) {
                    let _ = write!(line, " [expanded={expanded}]");
                }
            }
            "selected" => {
                if prop_value
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    line.push_str(" [selected]");
                }
            }
            _ => {}
        }
    }
}

/// Post-pass: assign `nth` indices to refs that share the same (role, name).
///
/// If only one ref has a given (role, name), its `nth` stays `None`.
/// If multiple refs share the same pair, they get `nth = Some(0)`, `Some(1)`, etc.
/// in the order they were inserted (which matches document order since we walk
/// the AX tree depth-first).
fn disambiguate_refs(refs: &mut RefMap) {
    // Collect ref ids grouped by (role, name), preserving insertion order
    let mut groups: HashMap<(String, String), Vec<String>> = HashMap::new();
    // We need stable iteration order — sort by ref id numerically (e1, e2, ...)
    let mut ids: Vec<String> = refs.keys().cloned().collect();
    ids.sort_by_key(|id| {
        id.strip_prefix('e')
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0)
    });

    for id in &ids {
        if let Some(r) = refs.get(id) {
            let key = (r.role.clone(), r.name.clone());
            groups.entry(key).or_default().push(id.clone());
        }
    }

    // Only assign nth when there are duplicates
    for group in groups.values() {
        if group.len() > 1 {
            for (i, id) in group.iter().enumerate() {
                if let Some(r) = refs.get_mut(id) {
                    r.nth = Some(i);
                }
            }
        }
    }
}

/// Extract a string value from an AX node property.
///
/// CDP returns `{ "role": { "type": "role", "value": "button" } }` —
/// this helper drills into the nested `value` field.
fn ax_str(node: &serde_json::Value, field: &str) -> String {
    node.get(field)
        .and_then(|v| v.get("value"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_nodes() -> Vec<serde_json::Value> {
        serde_json::from_str(
            r#"[
                {
                    "nodeId": "1",
                    "role": {"type": "role", "value": "WebArea"},
                    "name": {"type": "computedString", "value": "Example"},
                    "childIds": ["2", "3"]
                },
                {
                    "nodeId": "2",
                    "role": {"type": "role", "value": "heading"},
                    "name": {"type": "computedString", "value": "Example Domain"},
                    "backendDOMNodeId": 10,
                    "properties": [
                        {"name": "level", "value": {"type": "integer", "value": 1}}
                    ],
                    "childIds": []
                },
                {
                    "nodeId": "3",
                    "role": {"type": "role", "value": "link"},
                    "name": {"type": "computedString", "value": "More information..."},
                    "backendDOMNodeId": 20,
                    "properties": [
                        {"name": "focusable", "value": {"type": "boolean", "value": true}}
                    ],
                    "childIds": []
                }
            ]"#,
        )
        .expect("valid test JSON")
    }

    #[test]
    fn snapshot_assigns_refs() {
        let snap = build_snapshot(&sample_nodes(), &SnapshotOptions::default());
        assert_eq!(snap.ref_count(), 2);
        assert!(snap.get_ref("e1").is_some());
        assert!(snap.get_ref("e2").is_some());
    }

    #[test]
    fn snapshot_tree_format() {
        let snap = build_snapshot(&sample_nodes(), &SnapshotOptions::default());
        let tree = snap.tree();
        assert!(tree.contains("heading \"Example Domain\" [ref=e1] [level=1]"));
        assert!(tree.contains("link \"More information...\" [ref=e2]"));
    }

    #[test]
    fn interactive_only_filter() {
        let opts = SnapshotOptions::default().interactive_only(true);
        let snap = build_snapshot(&sample_nodes(), &opts);
        // heading is content, not interactive — but should still appear
        // since we only skip non-interactive non-content roles
        let tree = snap.tree();
        // link is interactive → must be present
        assert!(tree.contains("link"));
    }

    #[test]
    fn ref_metadata() {
        let snap = build_snapshot(&sample_nodes(), &SnapshotOptions::default());
        let link_ref = snap.get_ref("e2").expect("link ref exists");
        assert_eq!(link_ref.role, "link");
        assert_eq!(link_ref.name, "More information...");
        assert_eq!(link_ref.backend_node_id, 20);
        assert!(link_ref.focusable);
    }

    #[test]
    fn nth_disambiguation() {
        // Two links with the same name should get nth indices
        let nodes: Vec<serde_json::Value> = serde_json::from_str(
            r#"[
                {
                    "nodeId": "1",
                    "role": {"type": "role", "value": "WebArea"},
                    "name": {"type": "computedString", "value": "Test"},
                    "childIds": ["2", "3", "4"]
                },
                {
                    "nodeId": "2",
                    "role": {"type": "role", "value": "link"},
                    "name": {"type": "computedString", "value": "Click me"},
                    "backendDOMNodeId": 10,
                    "properties": [],
                    "childIds": []
                },
                {
                    "nodeId": "3",
                    "role": {"type": "role", "value": "link"},
                    "name": {"type": "computedString", "value": "Click me"},
                    "backendDOMNodeId": 20,
                    "properties": [],
                    "childIds": []
                },
                {
                    "nodeId": "4",
                    "role": {"type": "role", "value": "button"},
                    "name": {"type": "computedString", "value": "Submit"},
                    "backendDOMNodeId": 30,
                    "properties": [],
                    "childIds": []
                }
            ]"#,
        )
        .expect("valid JSON");

        let snap = build_snapshot(&nodes, &SnapshotOptions::default());
        assert_eq!(snap.ref_count(), 3);

        // Two links with same name → nth = Some(0) and Some(1)
        let r1 = snap.get_ref("e1").expect("first link");
        let r2 = snap.get_ref("e2").expect("second link");
        assert_eq!(r1.role, "link");
        assert_eq!(r2.role, "link");
        assert_eq!(r1.nth, Some(0));
        assert_eq!(r2.nth, Some(1));

        // Unique button → nth stays None
        let r3 = snap.get_ref("e3").expect("button");
        assert_eq!(r3.role, "button");
        assert_eq!(r3.nth, None);
    }
}
