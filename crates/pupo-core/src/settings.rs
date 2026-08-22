//! The Models settings page's tree, without a widget in sight.
//!
//! The checked set is pi's own `enabledModels` in `~/.pi/agent/settings.json` —
//! the setting behind its `--models` flag — rather than a preference of ours, so
//! the scope chosen here is the scope pi starts a session with and the one its
//! own picker cycles through. [`crate::pi::config`] holds that contract; this is
//! the list in front of it.
//!
//! The shape is a provider, then its models — except behind an aggregator, where
//! it is a provider, the upstream vendor, then the models. OpenRouter alone
//! lists hundreds of models under some sixty vendors, all named `vendor/model`;
//! flattened, one provider's catalogue buries every other, and the vendor a
//! model comes from is usually how you know whether you want it at all.
//!
//! The tree is stored flat, as a list of nodes carrying a depth. A node's
//! descendants are the nodes after it with a greater depth, up to the next one
//! at its own — which makes every operation here (tally, check a whole group,
//! hide what a filter missed) a walk over a slice rather than a recursion
//! through borrowed children that Rust would make a fight.

use serde_json::Value;

use crate::pi::config;

/// Above this many models the providers open closed: an aggregator lists
/// hundreds, and every other provider would be somewhere below them. Under it,
/// the whole catalogue is worth showing at once.
const EXPANDED_MAX: usize = 40;

/// One row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// 0 for a provider, 1 for a vendor or a model under a provider, 2 for a
    /// model under a vendor.
    pub depth: u8,
    /// What the row shows. A group's tally is added by [`ModelTree::rows`], not
    /// stored here — it changes with every click.
    pub label: String,
    /// What a filter matches against: the full `provider/id` and the display
    /// name, so a nested model still answers to its vendor after the tree has
    /// taken that prefix out of its label.
    pub name: String,
    /// The `provider/id` written into `enabledModels`, or "" for a group.
    pub reference: String,
    pub group: bool,
    pub checked: bool,
    pub expanded: bool,
    /// Cleared by a filter that this row and none of its descendants matched.
    pub matched: bool,
}

/// How a group's box is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Off,
    /// Some of what is under it, but not all.
    Partial,
    On,
}

/// One row as the page draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Index into the tree, which is what every method here takes.
    pub index: usize,
    pub depth: u8,
    pub label: String,
    pub group: bool,
    pub expanded: bool,
    pub state: Check,
}

#[derive(Debug, Clone, Default)]
pub struct ModelTree {
    nodes: Vec<Node>,
    /// The catalogue the tree was built from, kept for `orphan_patterns`.
    models: Vec<Value>,
    /// What the rows are counting: a catalogue counts models, and the fallback
    /// list counts the entries of a settings file.
    counts_models: bool,
}

/// The upstream vendor an aggregator's model id names — `moonshotai` in
/// `moonshotai/kimi-k3` — or "" for a plain id.
///
/// A leading slash is a path, not a vendor: llama.cpp names its models by the
/// file they were loaded from.
fn vendor(model: &Value) -> String {
    let id = model.get("id").and_then(Value::as_str).unwrap_or_default();
    match id.split_once('/') {
        Some((head, tail)) if !head.is_empty() && !tail.is_empty() => head.to_string(),
        _ => String::new(),
    }
}

fn field(model: &Value, key: &str) -> String {
    model
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl ModelTree {
    /// pi's catalogue, checked against the stored scope.
    pub fn from_models(models: &[Value], patterns: &[String]) -> Self {
        let models: Vec<Value> = models.iter().filter(|m| m.is_object()).cloned().collect();
        if models.is_empty() {
            return Self::from_patterns(patterns);
        }
        let scoped: Vec<String> = config::in_scope(&models, Some(patterns))
            .iter()
            .map(config::model_ref)
            .collect();
        let expanded = models.len() <= EXPANDED_MAX;

        // Providers in the order pi listed them: the catalogue's own order is
        // the one the picker shows, and re-sorting it here would make the two
        // pages disagree about where a provider lives.
        let mut providers: Vec<(String, Vec<Value>)> = Vec::new();
        for model in &models {
            let provider = match field(model, "provider") {
                name if name.is_empty() => "unknown".to_string(),
                name => name,
            };
            match providers.iter_mut().find(|(name, _)| *name == provider) {
                Some((_, list)) => list.push(model.clone()),
                None => providers.push((provider, vec![model.clone()])),
            }
        }

        let mut nodes = Vec::new();
        for (provider, models) in &providers {
            nodes.push(Node {
                depth: 0,
                label: provider.clone(),
                name: provider.to_lowercase(),
                reference: String::new(),
                group: true,
                checked: false,
                expanded,
                matched: true,
            });
            // One vendor is not a division worth a level of tree: it would put
            // every model of a single-vendor provider one indent further in,
            // under a heading that repeats the provider.
            let mut seen: Vec<String> = Vec::new();
            for model in models {
                let name = vendor(model);
                if !name.is_empty() && !seen.contains(&name) {
                    seen.push(name);
                }
            }
            let nested = seen.len() > 1;
            let mut opened: Vec<String> = Vec::new();
            for model in models {
                let vendor_name = if nested { vendor(model) } else { String::new() };
                if !vendor_name.is_empty() && !opened.contains(&vendor_name) {
                    opened.push(vendor_name.clone());
                    nodes.push(Node {
                        depth: 1,
                        label: vendor_name.clone(),
                        name: vendor_name.to_lowercase(),
                        reference: String::new(),
                        group: true,
                        checked: false,
                        expanded,
                        matched: true,
                    });
                }
                let reference = config::model_ref(model);
                nodes.push(Node {
                    depth: if vendor_name.is_empty() { 1 } else { 2 },
                    label: leaf_label(model, &vendor_name),
                    name: format!("{} {}", reference, field(model, "name")).to_lowercase(),
                    checked: scoped.contains(&reference),
                    reference,
                    group: false,
                    expanded: false,
                    matched: true,
                });
            }
        }
        Self {
            nodes,
            models,
            counts_models: true,
        }
    }

    /// The fallback with no catalogue to list: the stored scope itself, one row
    /// per pattern. Unchecking still works, which is what makes this worth
    /// showing rather than an empty page.
    pub fn from_patterns(patterns: &[String]) -> Self {
        let mut nodes = Vec::new();
        if !patterns.is_empty() {
            nodes.push(Node {
                depth: 0,
                label: "settings.json".to_string(),
                name: "settings.json".to_string(),
                reference: String::new(),
                group: true,
                checked: false,
                expanded: true,
                matched: true,
            });
            for pattern in patterns {
                nodes.push(Node {
                    depth: 1,
                    label: pattern.clone(),
                    name: pattern.to_lowercase(),
                    reference: pattern.clone(),
                    group: false,
                    checked: true,
                    expanded: false,
                    matched: true,
                });
            }
        }
        Self {
            nodes,
            models: Vec::new(),
            counts_models: false,
        }
    }

    /// The word the tally counts in. A page listing patterns is not counting
    /// models, and saying so would be counting the wrong thing.
    pub fn noun(&self, count: usize) -> &'static str {
        match (self.counts_models, count) {
            (true, 1) => "model",
            (true, _) => "models",
            (false, 1) => "entry",
            (false, _) => "entries",
        }
    }

    /// The half-open range of `index`'s descendants.
    fn descendants(&self, index: usize) -> std::ops::Range<usize> {
        let depth = self.nodes[index].depth;
        let mut end = index + 1;
        while end < self.nodes.len() && self.nodes[end].depth > depth {
            end += 1;
        }
        (index + 1)..end
    }

    /// How many leaves under `index` are checked, of how many there are.
    fn tally(&self, index: usize) -> (usize, usize) {
        let mut checked = 0;
        let mut total = 0;
        for node in &self.nodes[self.descendants(index)] {
            if !node.group {
                total += 1;
                checked += usize::from(node.checked);
            }
        }
        (checked, total)
    }

    fn state(&self, index: usize) -> Check {
        let node = &self.nodes[index];
        if !node.group {
            return if node.checked { Check::On } else { Check::Off };
        }
        match self.tally(index) {
            (0, _) => Check::Off,
            (checked, total) if checked == total => Check::On,
            _ => Check::Partial,
        }
    }

    /// The rows to draw: everything a filter kept, minus what a collapsed group
    /// is hiding.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        // The depth below which everything is inside something collapsed.
        let mut hidden_below: Option<u8> = None;
        for index in 0..self.nodes.len() {
            let node = &self.nodes[index];
            if let Some(depth) = hidden_below {
                if node.depth > depth {
                    continue;
                }
                hidden_below = None;
            }
            if !node.matched {
                continue;
            }
            if node.group && !node.expanded {
                hidden_below = Some(node.depth);
            }
            let label = if node.group {
                let (checked, total) = self.tally(index);
                format!("{}  ({checked}/{total} {})", node.label, self.noun(total))
            } else {
                node.label.clone()
            };
            rows.push(Row {
                index,
                depth: node.depth,
                label,
                group: node.group,
                expanded: node.expanded,
                state: self.state(index),
            });
        }
        rows
    }

    /// Flip a row. A group carries everything under it with it — and a group
    /// that is only partly checked fills up rather than emptying, because the
    /// click that reaches for a half-full box is the one asking for all of it.
    pub fn toggle(&mut self, index: usize) {
        if index >= self.nodes.len() {
            return;
        }
        if !self.nodes[index].group {
            self.nodes[index].checked = !self.nodes[index].checked;
            return;
        }
        let wanted = self.state(index) != Check::On;
        for at in self.descendants(index) {
            self.nodes[at].checked = wanted;
        }
    }

    pub fn set_expanded(&mut self, index: usize, open: bool) {
        if let Some(node) = self.nodes.get_mut(index) {
            if node.group {
                node.expanded = open;
            }
        }
    }

    pub fn set_all(&mut self, checked: bool) {
        for node in &mut self.nodes {
            if !node.group {
                node.checked = checked;
            }
        }
    }

    /// Hide what the query missed. A group survives if it matched itself or if
    /// anything under it did — a filter that hid the heading of a row it kept
    /// would leave the row floating with nothing to say where it came from.
    pub fn set_filter(&mut self, query: &str) {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            for node in &mut self.nodes {
                node.matched = true;
            }
            return;
        }
        for index in 0..self.nodes.len() {
            self.nodes[index].matched = self.nodes[index].name.contains(&needle);
        }
        for index in (0..self.nodes.len()).rev() {
            if !self.nodes[index].group || self.nodes[index].matched {
                continue;
            }
            let range = self.descendants(index);
            if self.nodes[range].iter().any(|node| node.matched) {
                self.nodes[index].matched = true;
            }
        }
        // A group the query itself named brings its whole catalogue with it.
        for index in 0..self.nodes.len() {
            if self.nodes[index].group && self.nodes[index].name.contains(&needle) {
                for at in self.descendants(index) {
                    self.nodes[at].matched = true;
                }
            }
        }
    }

    pub fn checked_refs(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|node| !node.group && node.checked && !node.reference.is_empty())
            .map(|node| node.reference.clone())
            .collect()
    }

    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// Stored patterns that no listed model matches — a scope entry for a
    /// provider whose credentials are gone for the moment, say.
    ///
    /// They are kept on every write: they are not ours to drop, and the page
    /// never showed them to be unchecked.
    pub fn orphan_patterns(&self, patterns: &[String]) -> Vec<String> {
        if self.models.is_empty() {
            return Vec::new();
        }
        patterns
            .iter()
            .filter(|pattern| {
                !self
                    .models
                    .iter()
                    .any(|model| config::matches_pattern(model, pattern))
            })
            .cloned()
            .collect()
    }
}

/// A model's row. Under its vendor the id sheds that prefix: the vendor is the
/// row above, and repeating it costs the width the id needs.
fn leaf_label(model: &Value, vendor_name: &str) -> String {
    let name = field(model, "name");
    let mut id = field(model, "id");
    if !vendor_name.is_empty() {
        if let Some(rest) = id.strip_prefix(&format!("{vendor_name}/")) {
            id = rest.to_string();
        }
    }
    if name.is_empty() || name == id {
        id
    } else {
        format!("{id}  ·  {name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(provider: &str, id: &str) -> Value {
        json!({ "provider": provider, "id": id, "name": "" })
    }

    fn labels(tree: &ModelTree) -> Vec<String> {
        tree.rows().iter().map(|row| row.label.clone()).collect()
    }

    #[test]
    fn a_provider_with_one_vendor_does_not_grow_a_level_for_it() {
        let models = vec![
            model("openai", "acme/one"),
            model("openai", "acme/two"),
        ];
        let tree = ModelTree::from_models(&models, &[]);
        let depths: Vec<u8> = tree.rows().iter().map(|row| row.depth).collect();
        assert_eq!(depths, vec![0, 1, 1]);
    }

    #[test]
    fn an_aggregator_nests_its_models_under_their_vendors() {
        let models = vec![
            model("openrouter", "acme/one"),
            model("openrouter", "other/two"),
        ];
        let tree = ModelTree::from_models(&models, &[]);
        let rows = tree.rows();
        assert_eq!(rows.iter().map(|r| r.depth).collect::<Vec<_>>(), vec![0, 1, 2, 1, 2]);
        // The vendor prefix is dropped from the row under its own vendor.
        assert_eq!(rows[2].label, "one");
    }

    #[test]
    fn a_leading_slash_is_a_path_rather_than_a_vendor() {
        assert_eq!(vendor(&model("local", "/models/qwen.gguf")), "");
        assert_eq!(vendor(&model("openrouter", "acme/one")), "acme");
    }

    #[test]
    fn an_empty_scope_checks_every_model() {
        let models = vec![model("openai", "one"), model("openai", "two")];
        let tree = ModelTree::from_models(&models, &[]);
        assert_eq!(tree.checked_refs(), vec!["openai/one", "openai/two"]);
    }

    #[test]
    fn a_stored_scope_checks_only_what_it_names() {
        let models = vec![model("openai", "one"), model("openai", "two")];
        let tree = ModelTree::from_models(&models, &["openai/one".to_string()]);
        assert_eq!(tree.checked_refs(), vec!["openai/one"]);
    }

    #[test]
    fn a_group_is_partial_until_everything_under_it_is_checked() {
        let models = vec![model("openai", "one"), model("openai", "two")];
        let mut tree = ModelTree::from_models(&models, &["openai/one".to_string()]);
        assert_eq!(tree.rows()[0].state, Check::Partial);
        // The click on a half-full box asks for all of it, not for none.
        tree.toggle(0);
        assert_eq!(tree.rows()[0].state, Check::On);
        tree.toggle(0);
        assert_eq!(tree.rows()[0].state, Check::Off);
    }

    #[test]
    fn a_group_row_counts_what_is_under_it() {
        let models = vec![model("openai", "one"), model("openai", "two")];
        let tree = ModelTree::from_models(&models, &["openai/one".to_string()]);
        assert_eq!(labels(&tree)[0], "openai  (1/2 models)");
    }

    #[test]
    fn a_collapsed_group_hides_what_is_under_it() {
        let models = vec![model("openai", "one"), model("openai", "two")];
        let mut tree = ModelTree::from_models(&models, &[]);
        tree.set_expanded(0, false);
        assert_eq!(tree.rows().len(), 1);
    }

    #[test]
    fn a_filter_keeps_the_heading_of_every_row_it_keeps() {
        let models = vec![model("openai", "one"), model("openai", "two")];
        let mut tree = ModelTree::from_models(&models, &[]);
        tree.set_filter("two");
        let rows = tree.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].group && rows[1].label == "two");
    }

    #[test]
    fn naming_a_provider_brings_its_whole_catalogue() {
        let models = vec![model("openai", "one"), model("anthropic", "two")];
        let mut tree = ModelTree::from_models(&models, &[]);
        tree.set_filter("openai");
        assert_eq!(tree.rows().len(), 2);
    }

    #[test]
    fn with_no_catalogue_the_stored_scope_is_the_list() {
        let tree = ModelTree::from_models(&[], &["openai/one".to_string()]);
        let rows = tree.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "settings.json  (1/1 entry)");
        assert_eq!(rows[1].label, "openai/one");
        assert_eq!(tree.checked_refs(), vec!["openai/one"]);
    }

    #[test]
    fn a_pattern_no_listed_model_matches_is_kept_rather_than_dropped() {
        let models = vec![model("openai", "one")];
        let tree = ModelTree::from_models(&models, &[]);
        let stored = vec!["openai/one".to_string(), "gone/two".to_string()];
        assert_eq!(tree.orphan_patterns(&stored), vec!["gone/two"]);
    }

    #[test]
    fn set_all_reaches_every_leaf_and_no_group() {
        let models = vec![model("openrouter", "acme/one"), model("openrouter", "b/two")];
        let mut tree = ModelTree::from_models(&models, &[]);
        tree.set_all(false);
        assert!(tree.checked_refs().is_empty());
        tree.set_all(true);
        assert_eq!(tree.checked_refs().len(), 2);
    }
}
