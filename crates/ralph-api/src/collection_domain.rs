mod yaml;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::errors::ApiError;
use crate::loop_support::now_ts;

use self::yaml::{export_collection_yaml, graph_from_yaml};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionCreateParams {
    pub name: String,
    pub description: Option<String>,
    pub graph: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionUpdateParams {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub graph: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionImportParams {
    pub yaml: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSummary {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionRecord {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub graph: GraphData,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub viewport: Viewport,
}

impl Default for GraphData {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            viewport: Viewport {
                x: 0.0,
                y: 0.0,
                zoom: 1.0,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub position: NodePosition,
    pub data: HatNodeData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodePosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HatNodeData {
    pub key: String,
    pub name: String,
    pub description: String,
    pub triggers_on: Vec<String>,
    pub publishes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CollectionSnapshot {
    collections: Vec<CollectionRecord>,
    id_counter: u64,
}

pub struct CollectionDomain {
    store_path: PathBuf,
    collections: BTreeMap<String, CollectionRecord>,
    id_counter: u64,
}

impl CollectionDomain {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        let store_path = workspace_root
            .as_ref()
            .join(".ralph/api/collections-v1.json");
        let mut domain = Self {
            store_path,
            collections: BTreeMap::new(),
            id_counter: 0,
        };
        domain.load();
        domain
    }

    pub fn list(&self) -> Vec<CollectionSummary> {
        let mut entries: Vec<_> = self
            .collections
            .values()
            .map(|collection| CollectionSummary {
                id: collection.id.clone(),
                name: collection.name.clone(),
                description: collection.description.clone(),
                created_at: collection.created_at.clone(),
                updated_at: collection.updated_at.clone(),
            })
            .collect();

        entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        entries
    }

    pub fn get(&self, id: &str) -> Result<CollectionRecord, ApiError> {
        self.collections
            .get(id)
            .cloned()
            .ok_or_else(|| collection_not_found_error(id))
    }

    pub fn create(&mut self, params: CollectionCreateParams) -> Result<CollectionRecord, ApiError> {
        let graph = params
            .graph
            .map(parse_graph)
            .transpose()?
            .unwrap_or_default();

        let now = now_ts();
        let id = self.next_collection_id();

        let record = CollectionRecord {
            id: id.clone(),
            name: params.name,
            description: params.description,
            graph,
            created_at: now.clone(),
            updated_at: now,
        };

        self.collections.insert(id.clone(), record);
        self.persist()?;
        self.get(&id)
    }

    pub fn update(&mut self, params: CollectionUpdateParams) -> Result<CollectionRecord, ApiError> {
        let record = self
            .collections
            .get_mut(&params.id)
            .ok_or_else(|| collection_not_found_error(&params.id))?;

        if let Some(name) = params.name {
            record.name = name;
        }

        if let Some(description) = params.description {
            record.description = Some(description);
        }

        if let Some(graph) = params.graph {
            record.graph = parse_graph(graph)?;
        }

        record.updated_at = now_ts();
        self.persist()?;
        self.get(&params.id)
    }

    pub fn delete(&mut self, id: &str) -> Result<(), ApiError> {
        if self.collections.remove(id).is_none() {
            return Err(collection_not_found_error(id));
        }

        self.persist()
    }

    pub fn import(&mut self, params: CollectionImportParams) -> Result<CollectionRecord, ApiError> {
        let graph = graph_from_yaml(&params.yaml)?;
        self.create(CollectionCreateParams {
            name: params.name,
            description: params.description,
            graph: Some(serde_json::to_value(graph).map_err(|error| {
                ApiError::internal(format!("failed serializing graph: {error}"))
            })?),
        })
    }

    pub fn export(&self, id: &str) -> Result<String, ApiError> {
        let collection = self.get(id)?;
        export_collection_yaml(&collection)
    }

    fn next_collection_id(&mut self) -> String {
        self.id_counter = self.id_counter.saturating_add(1);
        format!(
            "collection-{}-{:04x}",
            Utc::now().timestamp_millis(),
            self.id_counter
        )
    }

    fn load(&mut self) {
        if !self.store_path.exists() {
            return;
        }

        let content = match fs::read_to_string(&self.store_path) {
            Ok(content) => content,
            Err(error) => {
                warn!(
                    path = %self.store_path.display(),
                    %error,
                    "failed reading collection snapshot"
                );
                return;
            }
        };

        let snapshot: CollectionSnapshot = match serde_json::from_str(&content) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                warn!(
                    path = %self.store_path.display(),
                    %error,
                    "failed parsing collection snapshot"
                );
                return;
            }
        };

        self.collections = snapshot
            .collections
            .into_iter()
            .map(|collection| (collection.id.clone(), collection))
            .collect();
        self.id_counter = snapshot.id_counter;
    }

    fn persist(&self) -> Result<(), ApiError> {
        if let Some(parent) = self.store_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ApiError::internal(format!(
                    "failed creating collection snapshot directory '{}': {error}",
                    parent.display()
                ))
            })?;
        }

        let snapshot = CollectionSnapshot {
            collections: self.sorted_records(),
            id_counter: self.id_counter,
        };

        let payload = serde_json::to_string_pretty(&snapshot).map_err(|error| {
            ApiError::internal(format!("failed serializing collections snapshot: {error}"))
        })?;

        fs::write(&self.store_path, payload).map_err(|error| {
            ApiError::internal(format!(
                "failed writing collection snapshot '{}': {error}",
                self.store_path.display()
            ))
        })
    }

    fn sorted_records(&self) -> Vec<CollectionRecord> {
        let mut records: Vec<_> = self.collections.values().cloned().collect();
        records.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        records
    }
}

fn parse_graph(raw: Value) -> Result<GraphData, ApiError> {
    serde_json::from_value(raw)
        .map_err(|error| ApiError::invalid_params(format!("invalid collection graph: {error}")))
}

fn collection_not_found_error(collection_id: &str) -> ApiError {
    ApiError::collection_not_found(format!("Collection with id '{collection_id}' not found"))
        .with_details(serde_json::json!({ "collectionId": collection_id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use tempfile::TempDir;

    use crate::errors::RpcErrorCode;

    fn domain() -> (TempDir, CollectionDomain) {
        let temp = TempDir::new().expect("tempdir");
        let domain = CollectionDomain::new(temp.path());
        (temp, domain)
    }

    fn sample_graph_value() -> Value {
        json!({
            "nodes": [
                {
                    "id": "planner",
                    "type": "hatNode",
                    "position": { "x": 10.0, "y": 20.0 },
                    "data": {
                        "key": "planner",
                        "name": "Planner",
                        "description": "Plans the work",
                        "triggersOn": ["task.start"],
                        "publishes": ["task.ready"],
                        "instructions": "Plan carefully"
                    }
                }
            ],
            "edges": [],
            "viewport": { "x": 0.0, "y": 0.0, "zoom": 1.0 }
        })
    }

    fn sample_yaml() -> String {
        // Two hats linked via a shared event to exercise edge generation.
        r"
event_loop:
  completion_promise: LOOP_COMPLETE
  starting_event: task.start
  max_iterations: 50
cli:
  backend: claude
  prompt_mode: arg
hats:
  planner:
    name: Planner
    description: Plans things
    triggers:
      - task.start
    publishes:
      - task.ready
    instructions: Plan carefully
  builder:
    name: Builder
    description: Builds things
    triggers:
      - task.ready
    publishes:
      - LOOP_COMPLETE
"
        .to_string()
    }

    #[test]
    fn new_on_empty_workspace_loads_nothing() {
        let (_temp, domain) = domain();
        assert!(domain.list().is_empty());
        assert_eq!(domain.id_counter, 0);
    }

    #[test]
    fn create_without_graph_uses_default_graph() {
        let (_temp, mut domain) = domain();

        let record = domain
            .create(CollectionCreateParams {
                name: "Empty".to_string(),
                description: None,
                graph: None,
            })
            .expect("create");

        assert_eq!(record.name, "Empty");
        assert!(record.description.is_none());
        assert!(record.graph.nodes.is_empty());
        assert!(record.graph.edges.is_empty());
        assert!((record.graph.viewport.zoom - 1.0).abs() < f64::EPSILON);
        assert_eq!(record.created_at, record.updated_at);
        assert!(record.id.starts_with("collection-"));
    }

    #[test]
    fn create_with_graph_parses_and_preserves_fields() {
        let (_temp, mut domain) = domain();

        let record = domain
            .create(CollectionCreateParams {
                name: "Loop".to_string(),
                description: Some("desc".to_string()),
                graph: Some(sample_graph_value()),
            })
            .expect("create");

        assert_eq!(record.description.as_deref(), Some("desc"));
        assert_eq!(record.graph.nodes.len(), 1);
        let node = &record.graph.nodes[0];
        assert_eq!(node.id, "planner");
        assert_eq!(node.node_type, "hatNode");
        assert_eq!(node.data.key, "planner");
        assert_eq!(node.data.name, "Planner");
        assert_eq!(node.data.triggers_on, vec!["task.start".to_string()]);
        assert_eq!(node.data.publishes, vec!["task.ready".to_string()]);
        assert_eq!(
            node.data.instructions.as_deref(),
            Some("Plan carefully")
        );
    }

    #[test]
    fn create_rejects_invalid_graph_shape() {
        let (_temp, mut domain) = domain();

        let err = domain
            .create(CollectionCreateParams {
                name: "Bad".to_string(),
                description: None,
                graph: Some(json!({ "nodes": "not a list" })),
            })
            .expect_err("should reject invalid graph");

        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(err.message.contains("invalid collection graph"));
    }

    #[test]
    fn get_missing_returns_not_found_with_details() {
        let (_temp, domain) = domain();

        let err = domain
            .get("collection-missing")
            .expect_err("should be not found");

        assert_eq!(err.code, RpcErrorCode::CollectionNotFound);
        let details = err.details.expect("details");
        assert_eq!(details["collectionId"], json!("collection-missing"));
    }

    #[test]
    fn list_returns_summaries_sorted_by_name_then_id() {
        let (_temp, mut domain) = domain();

        let b = domain
            .create(CollectionCreateParams {
                name: "Beta".to_string(),
                description: None,
                graph: None,
            })
            .expect("create beta");
        let a = domain
            .create(CollectionCreateParams {
                name: "Alpha".to_string(),
                description: Some("first".to_string()),
                graph: None,
            })
            .expect("create alpha");

        let summaries = domain.list();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].id, a.id);
        assert_eq!(summaries[0].name, "Alpha");
        assert_eq!(summaries[0].description.as_deref(), Some("first"));
        assert_eq!(summaries[1].id, b.id);
        assert_eq!(summaries[1].name, "Beta");
    }

    #[test]
    fn update_partial_fields_leaves_others_untouched() {
        let (_temp, mut domain) = domain();
        let record = domain
            .create(CollectionCreateParams {
                name: "Original".to_string(),
                description: Some("keep".to_string()),
                graph: None,
            })
            .expect("create");

        // Rename only.
        let renamed = domain
            .update(CollectionUpdateParams {
                id: record.id.clone(),
                name: Some("Renamed".to_string()),
                description: None,
                graph: None,
            })
            .expect("update name");

        assert_eq!(renamed.name, "Renamed");
        assert_eq!(renamed.description.as_deref(), Some("keep"));
        assert_eq!(renamed.id, record.id);
        // updated_at is RFC3339 seconds — may equal created_at if within the
        // same second; assert it was at least set to something non-empty.
        assert!(!renamed.updated_at.is_empty());

        // Swap graph.
        let regraphed = domain
            .update(CollectionUpdateParams {
                id: record.id.clone(),
                name: None,
                description: None,
                graph: Some(sample_graph_value()),
            })
            .expect("update graph");
        assert_eq!(regraphed.graph.nodes.len(), 1);
        assert_eq!(regraphed.graph.nodes[0].data.key, "planner");
    }

    #[test]
    fn update_rejects_invalid_graph_without_mutating_record() {
        let (_temp, mut domain) = domain();
        let record = domain
            .create(CollectionCreateParams {
                name: "Immutable".to_string(),
                description: None,
                graph: Some(sample_graph_value()),
            })
            .expect("create");

        let err = domain
            .update(CollectionUpdateParams {
                id: record.id.clone(),
                name: None,
                description: None,
                graph: Some(json!({ "nodes": 42 })),
            })
            .expect_err("should reject");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);

        // Graph should still be the originally parsed one.
        let current = domain.get(&record.id).expect("still exists");
        assert_eq!(current.graph.nodes.len(), 1);
        assert_eq!(current.graph.nodes[0].id, "planner");
    }

    #[test]
    fn update_missing_returns_not_found() {
        let (_temp, mut domain) = domain();
        let err = domain
            .update(CollectionUpdateParams {
                id: "ghost".to_string(),
                name: Some("x".to_string()),
                description: None,
                graph: None,
            })
            .expect_err("should fail");
        assert_eq!(err.code, RpcErrorCode::CollectionNotFound);
    }

    #[test]
    fn delete_removes_record_and_missing_is_error() {
        let (_temp, mut domain) = domain();
        let record = domain
            .create(CollectionCreateParams {
                name: "Ephemeral".to_string(),
                description: None,
                graph: None,
            })
            .expect("create");

        domain.delete(&record.id).expect("delete");
        assert!(domain.list().is_empty());
        assert!(domain.get(&record.id).is_err());

        let err = domain.delete(&record.id).expect_err("already gone");
        assert_eq!(err.code, RpcErrorCode::CollectionNotFound);
    }

    #[test]
    fn persist_and_reload_preserves_collections_and_id_counter() {
        let temp = TempDir::new().expect("tempdir");
        let first_id;
        let second_id;
        {
            let mut domain = CollectionDomain::new(temp.path());
            first_id = domain
                .create(CollectionCreateParams {
                    name: "First".to_string(),
                    description: None,
                    graph: Some(sample_graph_value()),
                })
                .expect("first")
                .id;
            second_id = domain
                .create(CollectionCreateParams {
                    name: "Second".to_string(),
                    description: Some("2".to_string()),
                    graph: None,
                })
                .expect("second")
                .id;
        }

        let snapshot_path = temp.path().join(".ralph/api/collections-v1.json");
        assert!(snapshot_path.exists(), "snapshot file should be written");

        let reloaded = CollectionDomain::new(temp.path());
        let summaries = reloaded.list();
        assert_eq!(summaries.len(), 2);
        let ids: Vec<_> = summaries.iter().map(|s| s.id.clone()).collect();
        assert!(ids.contains(&first_id));
        assert!(ids.contains(&second_id));
        assert_eq!(reloaded.id_counter, 2);

        let first = reloaded.get(&first_id).expect("first reload");
        assert_eq!(first.graph.nodes.len(), 1);
        assert_eq!(first.graph.nodes[0].data.key, "planner");
    }

    #[test]
    fn load_ignores_corrupt_snapshot() {
        let temp = TempDir::new().expect("tempdir");
        let snapshot_path = temp.path().join(".ralph/api/collections-v1.json");
        fs::create_dir_all(snapshot_path.parent().unwrap()).unwrap();
        fs::write(&snapshot_path, "{ not valid json").unwrap();

        let domain = CollectionDomain::new(temp.path());
        // Corrupt file is logged and ignored — domain should start empty.
        assert!(domain.list().is_empty());
        assert_eq!(domain.id_counter, 0);
    }

    #[test]
    fn import_builds_nodes_and_cross_hat_edges() {
        let (_temp, mut domain) = domain();

        let record = domain
            .import(CollectionImportParams {
                yaml: sample_yaml(),
                name: "Imported".to_string(),
                description: Some("from yaml".to_string()),
            })
            .expect("import");

        // Two hats.
        assert_eq!(record.graph.nodes.len(), 2);
        let keys: Vec<_> = record
            .graph
            .nodes
            .iter()
            .map(|node| node.data.key.clone())
            .collect();
        assert!(keys.contains(&"planner".to_string()));
        assert!(keys.contains(&"builder".to_string()));

        // Exactly one edge — planner publishes task.ready which builder triggers on.
        // (task.start is only a trigger with no publisher; LOOP_COMPLETE is only
        // published with no subscriber.)
        assert_eq!(record.graph.edges.len(), 1);
        let edge = &record.graph.edges[0];
        assert_eq!(edge.source, "planner");
        assert_eq!(edge.target, "builder");
        assert_eq!(edge.label.as_deref(), Some("task.ready"));
        assert_eq!(edge.source_handle.as_deref(), Some("task.ready"));
        assert_eq!(edge.target_handle.as_deref(), Some("task.ready"));

        // Viewport override from yaml importer.
        assert!((record.graph.viewport.zoom - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn import_rejects_non_mapping_yaml() {
        let (_temp, mut domain) = domain();
        let err = domain
            .import(CollectionImportParams {
                yaml: "- just\n- a\n- list\n".to_string(),
                name: "bad".to_string(),
                description: None,
            })
            .expect_err("should reject");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(err.message.contains("must be a mapping"));
    }

    #[test]
    fn import_rejects_yaml_without_hats() {
        let (_temp, mut domain) = domain();
        let err = domain
            .import(CollectionImportParams {
                yaml: "event_loop:\n  completion_promise: x\n".to_string(),
                name: "bad".to_string(),
                description: None,
            })
            .expect_err("should reject");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(err.message.contains("must define hats"));
    }

    #[test]
    fn import_rejects_malformed_yaml() {
        let (_temp, mut domain) = domain();
        let err = domain
            .import(CollectionImportParams {
                yaml: ":\n\t: not yaml".to_string(),
                name: "bad".to_string(),
                description: None,
            })
            .expect_err("should reject");
        assert_eq!(err.code, RpcErrorCode::InvalidParams);
        assert!(err.message.contains("invalid YAML"));
    }

    #[test]
    fn export_emits_yaml_containing_hat_keys_and_events() {
        let (_temp, mut domain) = domain();
        let record = domain
            .import(CollectionImportParams {
                yaml: sample_yaml(),
                name: "Exported".to_string(),
                description: Some("round trip".to_string()),
            })
            .expect("import");

        let yaml = domain.export(&record.id).expect("export");
        assert!(yaml.contains("# Exported"));
        assert!(yaml.contains("# round trip"));
        assert!(yaml.contains("hats:"));
        assert!(yaml.contains("planner:"));
        assert!(yaml.contains("builder:"));
        assert!(yaml.contains("task.ready"));
        assert!(yaml.contains("event_loop:"));
    }

    #[test]
    fn export_missing_is_not_found() {
        let (_temp, domain) = domain();
        let err = domain.export("nope").expect_err("should fail");
        assert_eq!(err.code, RpcErrorCode::CollectionNotFound);
    }

    #[test]
    fn import_export_import_preserves_hat_keys_and_edges() {
        let (_temp, mut domain) = domain();
        let first = domain
            .import(CollectionImportParams {
                yaml: sample_yaml(),
                name: "Original".to_string(),
                description: None,
            })
            .expect("import 1");

        let exported = domain.export(&first.id).expect("export");
        let reimported = domain
            .import(CollectionImportParams {
                yaml: exported,
                name: "Reimported".to_string(),
                description: None,
            })
            .expect("import 2");

        let mut first_keys: Vec<_> = first
            .graph
            .nodes
            .iter()
            .map(|node| node.data.key.clone())
            .collect();
        first_keys.sort();
        let mut second_keys: Vec<_> = reimported
            .graph
            .nodes
            .iter()
            .map(|node| node.data.key.clone())
            .collect();
        second_keys.sort();
        assert_eq!(first_keys, second_keys);

        // The planner→builder edge is preserved across the round trip.
        let has_edge = reimported
            .graph
            .edges
            .iter()
            .any(|edge| edge.source == "planner" && edge.target == "builder");
        assert!(has_edge, "planner→builder edge should survive round trip");
    }

    #[test]
    fn id_counter_assigns_distinct_ids_monotonically() {
        let (_temp, mut domain) = domain();
        let a = domain
            .create(CollectionCreateParams {
                name: "A".to_string(),
                description: None,
                graph: None,
            })
            .expect("create a");
        let b = domain
            .create(CollectionCreateParams {
                name: "B".to_string(),
                description: None,
                graph: None,
            })
            .expect("create b");

        assert_ne!(a.id, b.id);
        // Suffix is a hex counter — ensure both ids carry one and they differ.
        let suffix = |id: &str| id.rsplit('-').next().map(str::to_string).unwrap();
        assert_eq!(suffix(&a.id), "0001");
        assert_eq!(suffix(&b.id), "0002");
    }
}
