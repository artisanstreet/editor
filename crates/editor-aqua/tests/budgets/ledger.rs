use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Budget {
    limit: u64,
    windows: Option<u64>,
    reason: String,
}

impl Budget {
    fn limit(&self) -> u64 {
        if cfg!(windows) {
            self.windows.unwrap_or(self.limit)
        } else {
            self.limit
        }
    }
}

pub(crate) struct Ledger {
    budgets: BTreeMap<String, Budget>,
    checked: Vec<(String, u64, String)>,
}

impl Ledger {
    pub(crate) fn load(budgets: &str) -> Self {
        Self {
            budgets: serde_json::from_str(budgets).expect("the budget file is valid JSON"),
            checked: Vec::new(),
        }
    }

    pub(crate) fn measure(&mut self, name: &str, measured: u64, detail: &str) {
        assert!(
            self.budgets.contains_key(name),
            "{name} has no budget in the budget file"
        );
        self.checked
            .push((name.to_owned(), measured, detail.to_owned()));
    }

    pub(crate) fn enforce(self) {
        let unchecked: Vec<_> = self
            .budgets
            .keys()
            .filter(|name| !self.checked.iter().any(|(checked, _, _)| checked == *name))
            .collect();
        assert!(
            unchecked.is_empty(),
            "budgets that nothing measures: {unchecked:?}"
        );
        let rows: Vec<_> = self
            .checked
            .iter()
            .map(|(name, measured, detail)| (name, *measured, detail, &self.budgets[name]))
            .collect();
        if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
            let mut summary = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(path)
                .expect("the job summary opens for appending");
            let mut table = String::from("| Budget | Measured | Limit |\n| --- | ---: | ---: |\n");
            for (name, measured, _, budget) in &rows {
                writeln!(table, "| `{name}` | {measured} | {} |", budget.limit())
                    .expect("formatting into a String cannot fail");
            }
            summary
                .write_all(table.as_bytes())
                .expect("the job summary accepts the budget table");
        }
        let overruns: Vec<_> = rows
            .iter()
            .filter(|(_, measured, _, budget)| *measured > budget.limit())
            .map(|(name, measured, detail, budget)| {
                if detail.is_empty() {
                    format!(
                        "{name} measured {measured} against a budget of {}: {}",
                        budget.limit(),
                        budget.reason
                    )
                } else {
                    format!(
                        "{name} measured {measured} against a budget of {} ({detail}): {}",
                        budget.limit(),
                        budget.reason
                    )
                }
            })
            .collect();
        assert!(
            overruns.is_empty(),
            "performance budgets exceeded:\n{}",
            overruns.join("\n")
        );
    }
}
