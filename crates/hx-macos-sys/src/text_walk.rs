//! Bounded traversal shared by native AX nodes and synthetic test trees.
use super::Text;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub(crate) trait Node: Clone {
    fn string(&self, key: &str) -> Option<String>;
    /// Return children in document order and whether the list was clipped.
    fn children(&self, limit: usize) -> (Vec<Self>, bool);
}
fn secure(node: &impl Node) -> bool {
    node.string("AXSubrole").as_deref() == Some("AXSecureTextField")
}

pub(crate) fn walk<N: Node>(root: N, node_cap: usize, deadline: Duration, byte_cap: usize) -> Text {
    if node_cap == 0 || byte_cap == 0 || deadline.is_zero() {
        return Text {
            text: String::new(),
            truncated: true,
            nodes: 0,
        };
    }
    let start = Instant::now();
    // Search breadth first using roles only. Browser chrome must not spend the
    // page's text/node budget before AXWebArea is even reached. Reserve most of
    // the deadline for collecting content, even in a pathological native tree.
    let mut queue = VecDeque::from([root.clone()]);
    let mut web = Vec::new();
    let mut searched = 0;
    let search_cap = node_cap.max(1024);
    let mut discovery_clipped = false;
    while !queue.is_empty() && searched < search_cap && start.elapsed() < deadline / 3 {
        let node = queue.pop_front().expect("nonempty queue");
        searched += 1;
        if secure(&node) {
            continue;
        }
        if node.string("AXRole").as_deref() == Some("AXWebArea") {
            web.push(node);
            continue;
        }
        let (children, clipped) = node.children(search_cap.saturating_sub(searched + queue.len()));
        discovery_clipped |= clipped;
        queue.extend(children);
    }
    discovery_clipped |= !queue.is_empty();
    let has_web = !web.is_empty();
    // Process every discovered web area before the chrome pass, which skips
    // those subtrees. Children are pushed in reverse to preserve reading order.
    let mut stack = vec![(root, true)];
    stack.extend(web.into_iter().rev().map(|n| (n, false)));
    let mut result = Text {
        text: String::new(),
        truncated: discovery_clipped,
        nodes: 0,
    };
    let mut last_line = String::new();
    while let Some((node, chrome)) = stack.pop() {
        if result.nodes >= node_cap || start.elapsed() >= deadline || result.text.len() >= byte_cap
        {
            result.truncated = true;
            break;
        }
        if secure(&node)
            || (chrome && has_web && node.string("AXRole").as_deref() == Some("AXWebArea"))
        {
            continue;
        }
        result.nodes += 1;
        let mut attributes = Vec::new();
        for key in ["AXTitle", "AXValue", "AXDescription"] {
            if start.elapsed() >= deadline {
                result.truncated = true;
                return result;
            }
            if let Some(value) = node.string(key) {
                if attributes.contains(&value) {
                    continue;
                }
                attributes.push(value.clone());
                for line in value.lines().map(str::trim).filter(|s| !s.is_empty()) {
                    // Adjacent nodes and multiple attributes often expose the
                    // same label. Keep intentional nonconsecutive repetitions.
                    if line == last_line {
                        continue;
                    }
                    last_line = line.to_owned();
                    for ch in line.chars().chain(std::iter::once('\n')) {
                        if result.text.len() + ch.len_utf8() > byte_cap {
                            result.truncated = true;
                            return result;
                        }
                        result.text.push(ch);
                    }
                }
            }
        }
        let (children, clipped) = node.children(node_cap.saturating_sub(result.nodes));
        result.truncated |= clipped;
        stack.extend(children.into_iter().rev().map(|n| (n, chrome)));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Default)]
    struct Fake {
        role: &'static str,
        value: &'static str,
        secure: bool,
        source: Option<&'static str>,
        children: Vec<Fake>,
    }
    impl Node for Fake {
        fn string(&self, key: &str) -> Option<String> {
            match key {
                "AXRole" => Some(self.role.into()),
                "AXSubrole" if self.secure => Some("AXSecureTextField".into()),
                "AXTitle" | "AXValue" | "AXDescription"
                    if self.source.is_none_or(|source| source == key) =>
                {
                    Some(self.value.into())
                }
                _ => None,
            }
        }
        fn children(&self, limit: usize) -> (Vec<Self>, bool) {
            (
                self.children.iter().take(limit).cloned().collect(),
                self.children.len() > limit,
            )
        }
    }
    fn label(value: &'static str) -> Fake {
        Fake {
            value,
            ..Fake::default()
        }
    }
    #[test]
    fn browser_chrome_cannot_spend_page_budget() {
        let page = Fake {
            role: "AXWebArea",
            children: vec![label("Synthetic post one"), label("Synthetic post two")],
            ..Fake::default()
        };
        let toolbar = Fake {
            children: vec![label("Close"); 300],
            ..Fake::default()
        };
        let root = Fake {
            children: vec![
                toolbar,
                Fake {
                    children: vec![page],
                    ..Fake::default()
                },
            ],
            ..Fake::default()
        };
        let result = walk(root, 16, Duration::from_secs(1), 100);
        assert!(
            result
                .text
                .starts_with("Synthetic post one\nSynthetic post two\n"),
            "{}",
            result.text
        );
        assert!(result.truncated);
    }
    #[test]
    fn deduplicates_labels_collapses_blanks_and_skips_password_subtrees() {
        let root = Fake {
            children: vec![
                label("Close"),
                label("Close"),
                label("\n\nBody\n\n\nNext\n"),
                Fake {
                    secure: true,
                    value: "secret",
                    children: vec![label("secret child")],
                    ..Fake::default()
                },
            ],
            ..Fake::default()
        };
        let result = walk(root, 100, Duration::from_secs(1), 1000);
        assert_eq!(result.text, "Close\nBody\nNext\n");
        assert!(!result.truncated);
    }
    #[test]
    fn deeply_nested_static_text_reads_values_and_descriptions() {
        let mut content = Fake {
            role: "AXWebArea",
            children: vec![
                Fake {
                    role: "AXStaticText",
                    value: "Value body",
                    source: Some("AXValue"),
                    ..Fake::default()
                },
                Fake {
                    role: "AXStaticText",
                    value: "Description body",
                    source: Some("AXDescription"),
                    ..Fake::default()
                },
            ],
            ..Fake::default()
        };
        for _ in 0..40 {
            content = Fake {
                children: vec![content],
                ..Fake::default()
            };
        }
        let result = walk(content, 64, Duration::from_secs(1), 100);
        assert_eq!(result.text, "Value body\nDescription body\n");
        assert!(!result.truncated);
    }
    #[test]
    fn utf8_and_zero_budgets_are_bounded() {
        let result = walk(label("ééé"), 10, Duration::from_secs(1), 5);
        assert_eq!(result.text, "éé");
        assert!(result.truncated);
        assert!(walk(label("text"), 0, Duration::from_secs(1), 100).truncated);
        assert!(walk(label("text"), 10, Duration::ZERO, 100).truncated);
    }
}
