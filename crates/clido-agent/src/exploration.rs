//! Exploration task types for multi-agent parallel exploration.
//!
//! Defines the types of exploration tasks that can be executed in parallel
//! by sub-agents, along with their results.

use std::path::PathBuf;
use std::time::Duration;

/// Types of exploration tasks that can be executed in parallel.
#[derive(Clone, Debug, PartialEq)]
pub enum ExplorationTask {
    /// Explore a specific directory and its contents.
    ExploreDirectory {
        /// Path to the directory to explore.
        path: PathBuf,
        /// Maximum depth to explore (0 = unlimited).
        depth: u8,
    },

    /// Find all files matching a pattern.
    FindFiles {
        /// Glob pattern to match (e.g., "**/*.rs").
        pattern: String,
        /// Maximum number of results to return.
        max_results: usize,
    },

    /// Search for a symbol/term across the codebase.
    SearchSymbol {
        /// Symbol or term to search for.
        symbol: String,
        /// File types to limit search to (e.g., ["rs", "toml"]).
        file_types: Vec<String>,
    },

    /// Analyze dependencies starting from entry points.
    AnalyzeDependencies {
        /// Entry point files to start analysis from.
        entry_points: Vec<PathBuf>,
    },

    /// Get a high-level overview of the codebase.
    CodebaseOverview {
        /// Maximum number of files to include in overview.
        max_files: usize,
    },

    /// Read and analyze specific files.
    ReadFiles {
        /// Paths to files to read.
        paths: Vec<PathBuf>,
    },

    /// Search for patterns using regex.
    GrepSearch {
        /// Regex pattern to search for.
        pattern: String,
        /// Optional path to limit search scope.
        path: Option<PathBuf>,
    },
}

impl ExplorationTask {
    /// Get a human-readable description of this task.
    pub fn description(&self) -> String {
        match self {
            ExplorationTask::ExploreDirectory { path, depth } => {
                format!("Explore directory {} (depth {})", path.display(), depth)
            }
            ExplorationTask::FindFiles {
                pattern,
                max_results,
            } => {
                format!("Find files matching '{}' (max {})", pattern, max_results)
            }
            ExplorationTask::SearchSymbol { symbol, file_types } => {
                if file_types.is_empty() {
                    format!("Search for symbol '{}'", symbol)
                } else {
                    format!("Search for symbol '{}' in {:?}", symbol, file_types)
                }
            }
            ExplorationTask::AnalyzeDependencies { entry_points } => {
                format!(
                    "Analyze dependencies from {} entry points",
                    entry_points.len()
                )
            }
            ExplorationTask::CodebaseOverview { max_files } => {
                format!("Get codebase overview (max {} files)", max_files)
            }
            ExplorationTask::ReadFiles { paths } => {
                format!("Read {} files", paths.len())
            }
            ExplorationTask::GrepSearch { pattern, path } => match path {
                Some(p) => format!("Grep '{}' in {}", pattern, p.display()),
                None => format!("Grep '{}'", pattern),
            },
        }
    }

    /// Estimate the complexity of this task (for task splitting decisions).
    pub fn complexity(&self) -> TaskComplexity {
        match self {
            ExplorationTask::ExploreDirectory { depth, .. } => {
                if *depth <= 2 {
                    TaskComplexity::Low
                } else if *depth <= 4 {
                    TaskComplexity::Medium
                } else {
                    TaskComplexity::High
                }
            }
            ExplorationTask::FindFiles { max_results, .. } => {
                if *max_results <= 10 {
                    TaskComplexity::Low
                } else if *max_results <= 50 {
                    TaskComplexity::Medium
                } else {
                    TaskComplexity::High
                }
            }
            ExplorationTask::SearchSymbol { .. } => TaskComplexity::Medium,
            ExplorationTask::AnalyzeDependencies { entry_points } => {
                if entry_points.len() <= 3 {
                    TaskComplexity::Medium
                } else {
                    TaskComplexity::High
                }
            }
            ExplorationTask::CodebaseOverview { max_files } => {
                if *max_files <= 20 {
                    TaskComplexity::Low
                } else if *max_files <= 50 {
                    TaskComplexity::Medium
                } else {
                    TaskComplexity::High
                }
            }
            ExplorationTask::ReadFiles { paths } => {
                if paths.len() <= 5 {
                    TaskComplexity::Low
                } else if paths.len() <= 15 {
                    TaskComplexity::Medium
                } else {
                    TaskComplexity::High
                }
            }
            ExplorationTask::GrepSearch { .. } => TaskComplexity::Medium,
        }
    }

    /// Check if this task can be parallelized.
    ///
    /// Some tasks have dependencies and must be executed sequentially.
    pub fn can_parallelize(&self) -> bool {
        match self {
            // These can always be parallelized
            ExplorationTask::ExploreDirectory { .. }
            | ExplorationTask::FindFiles { .. }
            | ExplorationTask::SearchSymbol { .. }
            | ExplorationTask::ReadFiles { .. }
            | ExplorationTask::GrepSearch { .. } => true,

            // These might have dependencies
            ExplorationTask::AnalyzeDependencies { .. } => true, // Can analyze in parallel
            ExplorationTask::CodebaseOverview { .. } => false,   // Should be single overview
        }
    }
}

/// Complexity level of an exploration task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskComplexity {
    /// Simple task, quick to complete.
    Low,
    /// Moderate complexity.
    Medium,
    /// Complex task, may take significant time.
    High,
}

/// Result of an exploration task.
#[derive(Clone, Debug)]
pub struct ExplorationResult {
    /// Unique ID of this result.
    pub task_id: uuid::Uuid,
    /// The task that was executed.
    pub task: ExplorationTask,
    /// Files that were read during exploration.
    pub files_read: Vec<PathBuf>,
    /// Key findings from exploration.
    pub findings: Vec<Finding>,
    /// Human-readable summary of results.
    pub summary: String,
    /// Token/cost usage for this exploration.
    pub usage: clido_core::Usage,
    /// Duration of the exploration.
    pub duration: Duration,
    /// Whether the exploration was successful.
    pub success: bool,
    /// Error message if exploration failed.
    pub error: Option<String>,
}

/// A single finding from exploration.
#[derive(Clone, Debug)]
pub struct Finding {
    /// Type of finding.
    pub kind: FindingKind,
    /// File path where finding was made.
    pub path: PathBuf,
    /// Description of the finding.
    pub description: String,
    /// Optional line number.
    pub line: Option<usize>,
    /// Relevance score (0.0 - 1.0).
    pub relevance: f32,
}

/// Type of finding.
#[derive(Clone, Debug, PartialEq)]
pub enum FindingKind {
    /// A file was found/discovered.
    File,
    /// A symbol (function, struct, etc.) was found.
    Symbol,
    /// A pattern match was found.
    Pattern,
    /// A dependency was identified.
    Dependency,
    /// A potential issue was found.
    Issue,
    /// General information.
    Info,
}

/// Task splitting heuristics.
pub struct TaskSplitter;

impl TaskSplitter {
    /// Split a complex task into multiple simpler tasks.
    ///
    /// Returns `None` if task should not be split.
    pub fn split(task: &ExplorationTask) -> Option<Vec<ExplorationTask>> {
        match task {
            ExplorationTask::ExploreDirectory { path, depth } if *depth > 3 => {
                // Split deep directory exploration by subdirectories
                // This would need actual filesystem access to do properly
                // For now, just split into two depth-limited tasks
                let mid_depth = *depth / 2;
                Some(vec![
                    ExplorationTask::ExploreDirectory {
                        path: path.clone(),
                        depth: mid_depth,
                    },
                    ExplorationTask::ExploreDirectory {
                        path: path.clone(),
                        depth: *depth,
                    },
                ])
            }
            ExplorationTask::ReadFiles { paths } if paths.len() > 10 => {
                // Split large file reads into chunks
                let chunks: Vec<_> = paths
                    .chunks(5)
                    .map(|chunk| ExplorationTask::ReadFiles {
                        paths: chunk.to_vec(),
                    })
                    .collect();
                Some(chunks)
            }
            _ => None, // Don't split other tasks
        }
    }

    /// Check if a list of tasks should be executed in parallel.
    ///
    /// Returns the recommended number of concurrent agents.
    pub fn recommended_concurrency(tasks: &[ExplorationTask]) -> usize {
        if tasks.is_empty() {
            return 0;
        }

        // Count complexity
        let high_complexity = tasks
            .iter()
            .filter(|t| t.complexity() == TaskComplexity::High)
            .count();
        let medium_complexity = tasks
            .iter()
            .filter(|t| t.complexity() == TaskComplexity::Medium)
            .count();

        // Recommend based on complexity mix
        if high_complexity > 0 {
            // Fewer concurrent agents for high complexity
            2.min(tasks.len())
        } else if medium_complexity > 3 {
            // Moderate concurrency for medium tasks
            3.min(tasks.len())
        } else {
            // More concurrency for simple tasks
            5.min(tasks.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_description() {
        let task = ExplorationTask::ExploreDirectory {
            path: PathBuf::from("/src"),
            depth: 3,
        };
        assert!(task.description().contains("Explore directory"));
        assert!(task.description().contains("/src"));
    }

    #[test]
    fn test_task_complexity() {
        let low = ExplorationTask::ExploreDirectory {
            path: PathBuf::from("/src"),
            depth: 1,
        };
        assert_eq!(low.complexity(), TaskComplexity::Low);

        let high = ExplorationTask::ExploreDirectory {
            path: PathBuf::from("/src"),
            depth: 5,
        };
        assert_eq!(high.complexity(), TaskComplexity::High);
    }

    #[test]
    fn test_task_splitting() {
        let task = ExplorationTask::ReadFiles {
            paths: (0..15)
                .map(|i| PathBuf::from(format!("/file{}.rs", i)))
                .collect(),
        };

        let split = TaskSplitter::split(&task);
        assert!(split.is_some());

        let tasks = split.unwrap();
        assert!(tasks.len() > 1);
    }

    #[test]
    fn test_recommended_concurrency() {
        let simple_tasks: Vec<_> = (0..10)
            .map(|_| ExplorationTask::ReadFiles {
                paths: vec![PathBuf::from("/file.rs")],
            })
            .collect();

        let concurrency = TaskSplitter::recommended_concurrency(&simple_tasks);
        assert_eq!(concurrency, 5); // Capped at 5
    }
}
