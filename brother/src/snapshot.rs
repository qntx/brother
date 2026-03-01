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

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.tree)
    }
}

/// Map from ref id to element metadata.
pub type RefMap = HashMap<String, Ref>;

/// Metadata for a single referenced element in the accessibility tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ref {
    /// ARIA role (e.g. `"button"`, `"link"`, `"textbox"`).
    pub role: String,
    /// Accessible name (e.g. button label, link text).
    pub name: String,
    /// CDP backend node id for direct element targeting.
    pub backend_node_id: i64,
    /// Whether this element is focusable.
    pub focusable: bool,
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
}

// ---------------------------------------------------------------------------
// Accessibility tree roles
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// AX tree building from CDP response
// ---------------------------------------------------------------------------

/// Build a [`Snapshot`] from the raw CDP accessibility tree nodes.
///
/// `nodes` should come from `Accessibility.getFullAXTree` CDP response.
pub fn build_snapshot(nodes: &[serde_json::Value], options: &SnapshotOptions) -> Snapshot {
    let mut ref_counter: u32 = 0;
    let mut refs = RefMap::new();
    let mut lines = Vec::new();

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
        render_node(
            root,
            &node_map,
            options,
            0,
            &mut ref_counter,
            &mut refs,
            &mut lines,
        );
    }

    Snapshot::new(lines.join("\n"), refs)
}

/// Recursively render a single AX node into the output lines.
#[allow(clippy::too_many_arguments)]
fn render_node(
    node_id: &str,
    node_map: &HashMap<String, &serde_json::Value>,
    options: &SnapshotOptions,
    depth: usize,
    ref_counter: &mut u32,
    refs: &mut RefMap,
    lines: &mut Vec<String>,
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
        render_children(node, node_map, options, depth, ref_counter, refs, lines);
        return;
    }

    let interactive = is_interactive(&role);
    let content = is_content(&role);
    let structural = is_structural(&role);

    // Interactive-only filter
    if options.interactive_only && !interactive {
        // Still recurse — interactive elements may be nested inside non-interactive containers
        render_children(node, node_map, options, depth, ref_counter, refs, lines);
        return;
    }

    // Compact mode: skip empty structural nodes
    if options.compact && structural && name.is_empty() {
        render_children(node, node_map, options, depth, ref_counter, refs, lines);
        return;
    }

    // Assign ref if interactive or content-bearing
    let ref_id = if interactive || content {
        *ref_counter += 1;
        let id = format!("e{ref_counter}");

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

        refs.insert(
            id.clone(),
            Ref {
                role: role.clone(),
                name: name.clone(),
                backend_node_id,
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

    lines.push(line);

    // Recurse into children
    render_children(node, node_map, options, depth + 1, ref_counter, refs, lines);
}

/// Render all children of a node.
fn render_children(
    node: &serde_json::Value,
    node_map: &HashMap<String, &serde_json::Value>,
    options: &SnapshotOptions,
    depth: usize,
    ref_counter: &mut u32,
    refs: &mut RefMap,
    lines: &mut Vec<String>,
) {
    let Some(children) = node.get("childIds").and_then(serde_json::Value::as_array) else {
        return;
    };
    for child_id in children {
        if let Some(id) = child_id.as_str() {
            render_node(id, node_map, options, depth, ref_counter, refs, lines);
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
}
