use std::collections::BTreeSet;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;

use crate::protocol::FileChange;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileSnapshot {
    pub(crate) existed: bool,
    pub(crate) content: String,
}

impl FileSnapshot {
    pub(crate) fn missing() -> Self {
        Self {
            existed: false,
            content: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnCheckpointRecord {
    pub(crate) turn_id: String,
    pub(crate) before: HashMap<PathBuf, FileSnapshot>,
    pub(crate) after: HashMap<PathBuf, FileSnapshot>,
}

#[derive(Debug, Clone)]
struct ActiveTurnCheckpoint {
    turn_id: String,
    before: HashMap<PathBuf, FileSnapshot>,
    after: HashMap<PathBuf, FileSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RollbackPlan {
    pub(crate) records: Vec<TurnCheckpointRecord>,
    pub(crate) changed_file_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnCheckpointError {
    MissingActiveTurn { turn_id: String },
    TurnMismatch { expected: String, actual: String },
    NoCheckpointHistory { requested_turns: u32 },
    Conflict { path: PathBuf },
    Io { path: PathBuf, message: String },
}

impl fmt::Display for TurnCheckpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingActiveTurn { turn_id } => {
                write!(f, "当前回合 `{turn_id}` 没有可用的代码检查点")
            }
            Self::TurnMismatch { expected, actual } => {
                write!(f, "检查点回合不匹配：期望 `{expected}`，实际 `{actual}`")
            }
            Self::NoCheckpointHistory { requested_turns } => {
                write!(
                    f,
                    "当前会话内没有足够的代码检查点，无法回退最近 {requested_turns} 轮代码"
                )
            }
            Self::Conflict { path } => {
                write!(
                    f,
                    "检测到文件已被后续手动修改，停止回退：{}",
                    path.display()
                )
            }
            Self::Io { path, message } => {
                write!(f, "处理文件 `{}` 时失败：{message}", path.display())
            }
        }
    }
}

impl std::error::Error for TurnCheckpointError {}

#[derive(Debug, Default)]
pub(crate) struct TurnCheckpointManager {
    active: Option<ActiveTurnCheckpoint>,
    records: Vec<TurnCheckpointRecord>,
}

impl TurnCheckpointManager {
    pub(crate) fn begin_turn(&mut self, turn_id: String) {
        self.active = Some(ActiveTurnCheckpoint {
            turn_id,
            before: HashMap::new(),
            after: HashMap::new(),
        });
    }

    pub(crate) fn finish_turn(&mut self, turn_id: &str) -> Result<(), TurnCheckpointError> {
        match self.active.take() {
            Some(active) if active.turn_id == turn_id => {
                self.records.push(TurnCheckpointRecord {
                    turn_id: active.turn_id,
                    before: active.before,
                    after: active.after,
                });
                Ok(())
            }
            Some(active) => {
                self.active = Some(active.clone());
                Err(TurnCheckpointError::TurnMismatch {
                    expected: active.turn_id,
                    actual: turn_id.to_string(),
                })
            }
            None => Ok(()),
        }
    }

    pub(crate) fn capture_before(
        &mut self,
        turn_id: &str,
        snapshots: HashMap<PathBuf, FileSnapshot>,
    ) -> Result<(), TurnCheckpointError> {
        let active =
            self.active
                .as_mut()
                .ok_or_else(|| TurnCheckpointError::MissingActiveTurn {
                    turn_id: turn_id.to_string(),
                })?;
        if active.turn_id != turn_id {
            return Err(TurnCheckpointError::TurnMismatch {
                expected: active.turn_id.clone(),
                actual: turn_id.to_string(),
            });
        }

        for (path, snapshot) in snapshots {
            active.before.entry(path).or_insert(snapshot);
        }
        Ok(())
    }

    pub(crate) fn capture_after(
        &mut self,
        turn_id: &str,
        snapshots: HashMap<PathBuf, FileSnapshot>,
    ) -> Result<(), TurnCheckpointError> {
        let active =
            self.active
                .as_mut()
                .ok_or_else(|| TurnCheckpointError::MissingActiveTurn {
                    turn_id: turn_id.to_string(),
                })?;
        if active.turn_id != turn_id {
            return Err(TurnCheckpointError::TurnMismatch {
                expected: active.turn_id.clone(),
                actual: turn_id.to_string(),
            });
        }

        active.after.extend(snapshots);
        Ok(())
    }

    pub(crate) fn plan_rollback(
        &self,
        requested_turns: u32,
    ) -> Result<RollbackPlan, TurnCheckpointError> {
        let requested_turns_usize = usize::try_from(requested_turns).unwrap_or(usize::MAX);
        if requested_turns_usize == 0 || requested_turns_usize > self.records.len() {
            return Err(TurnCheckpointError::NoCheckpointHistory { requested_turns });
        }

        let records = self.records[self.records.len() - requested_turns_usize..].to_vec();
        let changed_file_count = records
            .iter()
            .flat_map(|record| {
                record
                    .before
                    .keys()
                    .chain(record.after.keys())
                    .cloned()
                    .collect::<BTreeSet<_>>()
            })
            .collect::<BTreeSet<_>>()
            .len();

        Ok(RollbackPlan {
            records,
            changed_file_count,
        })
    }

    pub(crate) fn commit_rollback(
        &mut self,
        requested_turns: u32,
    ) -> Result<(), TurnCheckpointError> {
        let requested_turns_usize = usize::try_from(requested_turns).unwrap_or(usize::MAX);
        if requested_turns_usize == 0 || requested_turns_usize > self.records.len() {
            return Err(TurnCheckpointError::NoCheckpointHistory { requested_turns });
        }
        let keep = self.records.len().saturating_sub(requested_turns_usize);
        self.records.truncate(keep);
        Ok(())
    }

    pub(crate) fn discard_last_turns(&mut self, requested_turns: u32) {
        let requested_turns_usize = usize::try_from(requested_turns).unwrap_or(usize::MAX);
        let keep = self.records.len().saturating_sub(requested_turns_usize);
        self.records.truncate(keep);
    }

    #[cfg(test)]
    pub(crate) fn record_count(&self) -> usize {
        self.records.len()
    }
}

pub(crate) fn touched_paths(
    cwd: &Path,
    changes: &HashMap<PathBuf, FileChange>,
) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for (path, change) in changes {
        paths.insert(resolve_path(cwd, path));
        if let FileChange::Update {
            move_path: Some(move_path),
            ..
        } = change
        {
            paths.insert(resolve_path(cwd, move_path));
        }
    }
    paths
}

pub(crate) fn resolve_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touched_paths_tracks_move_destination() {
        let cwd = PathBuf::from("/tmp/project");
        let mut changes = HashMap::new();
        changes.insert(
            PathBuf::from("src/old.rs"),
            FileChange::Update {
                unified_diff: "@@".to_string(),
                move_path: Some(PathBuf::from("src/new.rs")),
            },
        );

        let paths = touched_paths(&cwd, &changes);

        assert!(paths.contains(&cwd.join("src/old.rs")));
        assert!(paths.contains(&cwd.join("src/new.rs")));
    }

    #[test]
    fn rollback_plan_uses_last_n_records() {
        let mut manager = TurnCheckpointManager::default();
        manager.begin_turn("turn-1".to_string());
        manager.finish_turn("turn-1").expect("finish turn-1");
        manager.begin_turn("turn-2".to_string());
        manager.finish_turn("turn-2").expect("finish turn-2");

        let plan = manager.plan_rollback(1).expect("rollback plan");

        assert_eq!(plan.records.len(), 1);
        assert_eq!(plan.records[0].turn_id, "turn-2");
    }

    #[test]
    fn commit_rollback_truncates_checkpoint_history() {
        let mut manager = TurnCheckpointManager::default();
        manager.begin_turn("turn-1".to_string());
        manager.finish_turn("turn-1").expect("finish turn-1");
        manager.begin_turn("turn-2".to_string());
        manager.finish_turn("turn-2").expect("finish turn-2");

        manager.commit_rollback(1).expect("commit rollback");

        assert_eq!(manager.record_count(), 1);
        let plan = manager.plan_rollback(1).expect("remaining plan");
        assert_eq!(plan.records[0].turn_id, "turn-1");
    }
}

// 编号（如：1）：新增
// 主要修改内容：新增会话内代码检查点管理器，负责记录每轮对话的文件前态、后态与回退计划。
// 修改目的：为非 Git 的代码回退提供基础数据结构，并让 Esc-Esc 回退流程可以按轮次恢复文件。
