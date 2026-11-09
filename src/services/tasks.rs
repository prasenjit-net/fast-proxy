use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: u64,
    pub title: String,
    pub done: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct NewTask {
    #[serde(default)]
    pub title: String,
}

/// Example service: an in-memory task list. It exists to demonstrate
/// the REST + validation + error-handling patterns; replace the storage
/// with a database while keeping the same interface.
pub struct TaskStore {
    items: RwLock<Vec<Task>>,
    next_id: AtomicU64,
}

impl TaskStore {
    pub fn with_examples() -> Self {
        let seed = [
            "Read the template README",
            "Wire up a real database",
            "Ship something",
        ];
        let items: Vec<Task> = seed
            .iter()
            .enumerate()
            .map(|(i, title)| Task {
                id: i as u64 + 1,
                title: (*title).to_string(),
                done: i == 0,
                created_at: Utc::now(),
            })
            .collect();
        Self {
            items: RwLock::new(items),
            next_id: AtomicU64::new(seed.len() as u64 + 1),
        }
    }

    pub async fn list(&self) -> Vec<Task> {
        self.items.read().await.clone()
    }

    pub async fn create(&self, title: &str) -> AppResult<Task> {
        let title = title.trim();
        if title.is_empty() {
            return Err(AppError::BadRequest("task title must not be empty".into()));
        }
        if title.len() > 120 {
            return Err(AppError::BadRequest(
                "task title must be 120 characters or fewer".into(),
            ));
        }
        let task = Task {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            title: title.to_string(),
            done: false,
            created_at: Utc::now(),
        };
        self.items.write().await.insert(0, task.clone());
        Ok(task)
    }

    pub async fn toggle(&self, id: u64) -> AppResult<Task> {
        let mut items = self.items.write().await;
        let task = items
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| AppError::NotFound(format!("task {id} does not exist")))?;
        task.done = !task.done;
        Ok(task.clone())
    }

    pub async fn delete(&self, id: u64) -> AppResult<Task> {
        let mut items = self.items.write().await;
        let index = items
            .iter()
            .position(|task| task.id == id)
            .ok_or_else(|| AppError::NotFound(format!("task {id} does not exist")))?;
        Ok(items.remove(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn seeds_three_tasks_with_the_first_done() {
        let store = TaskStore::with_examples();
        let tasks = store.list().await;
        assert_eq!(tasks.len(), 3);
        assert!(tasks[0].done);
        assert!(!tasks[1].done);
    }

    #[tokio::test]
    async fn create_trims_whitespace_and_prepends() {
        let store = TaskStore::with_examples();
        let task = store.create("  Write more tests  ").await.unwrap();
        assert_eq!(task.title, "Write more tests");
        assert!(!task.done);

        let tasks = store.list().await;
        assert_eq!(tasks.len(), 4);
        assert_eq!(tasks[0].id, task.id, "newest task should be listed first");
    }

    #[tokio::test]
    async fn create_rejects_empty_or_whitespace_only_title() {
        let store = TaskStore::with_examples();
        let err = store.create("   ").await.unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn create_rejects_titles_over_120_chars() {
        let store = TaskStore::with_examples();
        let long_title = "x".repeat(121);
        let err = store.create(&long_title).await.unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));

        // Exactly 120 is still allowed.
        let ok_title = "x".repeat(120);
        assert!(store.create(&ok_title).await.is_ok());
    }

    #[tokio::test]
    async fn ids_never_repeat_even_after_deletion() {
        let store = TaskStore::with_examples();
        let first = store.create("first").await.unwrap();
        store.delete(first.id).await.unwrap();
        let second = store.create("second").await.unwrap();
        assert_ne!(first.id, second.id);
    }

    #[tokio::test]
    async fn toggle_flips_done_and_round_trips() {
        let store = TaskStore::with_examples();
        let before = store.list().await[1].done; // seed task 2 starts false
        let toggled = store.toggle(2).await.unwrap();
        assert_eq!(toggled.done, !before);
        let toggled_back = store.toggle(2).await.unwrap();
        assert_eq!(toggled_back.done, before);
    }

    #[tokio::test]
    async fn toggle_and_delete_unknown_id_are_not_found() {
        let store = TaskStore::with_examples();
        assert!(matches!(
            store.toggle(9999).await.unwrap_err(),
            AppError::NotFound(_)
        ));
        assert!(matches!(
            store.delete(9999).await.unwrap_err(),
            AppError::NotFound(_)
        ));
    }

    #[tokio::test]
    async fn delete_removes_exactly_that_task() {
        let store = TaskStore::with_examples();
        store.delete(2).await.unwrap();
        let tasks = store.list().await;
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|t| t.id != 2));
    }
}
