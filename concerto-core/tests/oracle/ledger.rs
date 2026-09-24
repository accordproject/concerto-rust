//! The owning task of each TS member, from the seam ledger
//! (`migration/ledger/SEAM_LEDGER.tsv` in the `concerto` checkout, the
//! `planned_task` column), so that an `unsupported` verdict can say which
//! Phase 2/3 task will add the op's dispatch entry (PORTING.md 6.2).
//!
//! The ledger is optional: the harness reads it from the checkout the
//! corpus lives in (`<fixtures>/../../ledger/`), and a reason simply omits
//! the task when it is not there.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Default)]
pub struct Ledger {
    /// `<Class>.<member>` (`constructor` spelled `new`, as ops spell it) to
    /// its planned task.
    members: HashMap<String, String>,
    /// `<Class>` to the planned task of its members, when they all share one.
    classes: HashMap<String, Option<String>>,
}

impl Ledger {
    pub fn load(fixtures_dir: &Path) -> Self {
        let Some(path) = fixtures_dir
            .parent()
            .and_then(Path::parent)
            .map(|migration| migration.join("ledger").join("SEAM_LEDGER.tsv"))
        else {
            return Self::default();
        };
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let mut lines = text.lines();
        let Some(header) = lines.next() else {
            return Self::default();
        };
        let columns: Vec<&str> = header.split('\t').collect();
        let col = |name: &str| columns.iter().position(|c| *c == name);
        let (Some(class_col), Some(member_col), Some(task_col)) =
            (col("class"), col("member"), col("planned_task"))
        else {
            return Self::default();
        };
        let mut ledger = Self::default();
        for line in lines {
            let fields: Vec<&str> = line.split('\t').collect();
            let (Some(class), Some(member), Some(task)) = (
                fields.get(class_col),
                fields.get(member_col),
                fields.get(task_col),
            ) else {
                continue;
            };
            if class.is_empty() || task.is_empty() || *task == "-" {
                continue;
            }
            let member = if *member == "constructor" {
                "new"
            } else {
                member
            };
            ledger
                .members
                .insert(format!("{class}.{member}"), (*task).to_string());
            ledger
                .classes
                .entry((*class).to_string())
                .and_modify(|shared| {
                    if shared.as_deref() != Some(*task) {
                        *shared = None;
                    }
                })
                .or_insert_with(|| Some((*task).to_string()));
        }
        ledger
    }

    /// The planned task for an op, `" (ledger: P2-03)"`, or `""`.
    pub fn owner(&self, op: &str) -> String {
        let task = self.members.get(op).cloned().or_else(|| {
            let class = op.split('.').next()?;
            self.classes.get(class).cloned().flatten()
        });
        task.map(|t| format!(" (ledger: {t})")).unwrap_or_default()
    }
}
