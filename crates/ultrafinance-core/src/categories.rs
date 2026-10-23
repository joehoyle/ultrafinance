//! Portable Foursquare taxonomy snapshot. PostgreSQL catalog reads resolve the live
//! taxonomy from the database. Provider IDs are opaque, never inferred from names.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::LazyLock};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CategoryNode {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Category {
    /// ID in the Foursquare Places OS taxonomy.
    pub id: String,
    /// Missing when a provider ID has no known taxonomy name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Root through leaf, including this category. Empty for unknown IDs.
    pub path: Vec<CategoryNode>,
}

#[derive(Deserialize)]
struct Row {
    category_id: String,
    category_name: String,
    category_label: String,
}

static TAXONOMY: LazyLock<BTreeMap<String, Category>> = LazyLock::new(|| {
    let rows: Vec<Row> =
        csv::Reader::from_reader(include_bytes!("../data/foursquare-categories.csv").as_slice())
            .deserialize()
            .collect::<Result<_, _>>()
            .expect("bundled Foursquare taxonomy must be valid CSV");
    let labels: BTreeMap<_, _> = rows.iter().map(|row| (&row.category_label, row)).collect();
    rows.iter()
        .map(|row| {
            let mut label = String::new();
            let path = row
                .category_label
                .split(" > ")
                .map(|part| {
                    if !label.is_empty() {
                        label.push_str(" > ");
                    }
                    label.push_str(part);
                    let ancestor = labels
                        .get(&label)
                        .expect("bundled taxonomy must contain ancestors");
                    CategoryNode {
                        id: ancestor.category_id.clone(),
                        name: ancestor.category_name.clone(),
                    }
                })
                .collect();
            (
                row.category_id.clone(),
                Category {
                    id: row.category_id.clone(),
                    name: Some(row.category_name.clone()),
                    path,
                },
            )
        })
        .collect()
});

/// Resolve assignments deterministically, preserving unrecognized provider IDs.
pub fn resolve(ids: &[String]) -> Vec<Category> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .map(|id| {
            TAXONOMY.get(&id).cloned().unwrap_or(Category {
                id,
                name: None,
                path: vec![],
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchy_has_valid_ancestors_and_leaf_ids() {
        assert_eq!(TAXONOMY.len(), 1279);
        for category in TAXONOMY.values() {
            assert_eq!(category.path.last().unwrap().id, category.id);
            for (depth, node) in category.path.iter().enumerate() {
                assert_eq!(TAXONOMY[&node.id].path, category.path[..=depth]);
            }
        }
        let attraction = &TAXONOMY["5109983191d435c0d71c2bb1"];
        assert_eq!(
            attraction
                .path
                .iter()
                .map(|n| n.name.as_str())
                .collect::<Vec<_>>(),
            ["Arts and Entertainment", "Amusement Park", "Attraction"]
        );
    }

    #[test]
    fn unknown_ids_survive_without_invented_names() {
        let categories = resolve(&["new-id".into(), "new-id".into()]);
        assert_eq!(categories.len(), 1);
        assert_eq!(categories[0].id, "new-id");
        assert!(categories[0].name.is_none());
        assert!(categories[0].path.is_empty());
    }
}
