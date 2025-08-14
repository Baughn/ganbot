# Jujutsu (jj) Development Guide

*Last updated: August 2025 - Based on official Jujutsu documentation*

## Table of Contents

1. [Introduction](#introduction)
2. [Key Concepts](#key-concepts)
3. [Basic Operations](#basic-operations)
4. [History Management](#history-management)
5. [Branch Management](#branch-management)
6. [Remote Operations](#remote-operations)
7. [Conflict Resolution](#conflict-resolution)
8. [Advanced Features](#advanced-features)
9. [Common Workflows](#common-workflows)
10. [Migration from Git](#migration-from-git)
11. [Configuration](#configuration)

## Introduction

Jujutsu (jj) is a powerful version control system that provides a more intuitive and flexible approach to managing code history compared to traditional systems. It's designed to be "Git-compatible" while offering significant improvements in workflow and usability.

**Key Benefits:**
- Working copy is automatically managed as commits
- No staging area complexity
- Conflicts are first-class objects that can be committed
- Automatic rebasing of descendant commits
- Comprehensive history rewriting capabilities
- Safe concurrent operations with operation logging

**Status:** Jujutsu is experimental but "fairly feature-complete." It's recommended for developers comfortable with trying new tools and adapting workflows.

## Key Concepts

### Working Copy as a Commit

Unlike Git, Jujutsu treats your working directory as an actual commit that gets automatically amended:

- **No dirty working directory**: Changes are automatically committed
- **No need for `git stash`**: The working copy is always clean from a version control perspective
- **Automatic tracking**: New files are implicitly tracked and committed

### Change IDs vs Commit IDs

Jujutsu introduces "change IDs" that remain stable even when commits are rewritten:

- **Change ID**: Stable identifier for a logical change
- **Commit ID**: Traditional hash that changes when commit is modified
- **Benefits**: Track changes across history rewriting operations

### No Staging Area

There's no index or staging area concept:

- All changes in working copy are included in commits
- Use commits instead of staging for managing partial changes
- Excellent support for moving changes between commits after the fact

## Basic Operations

### Repository Initialization

```bash
# Initialize a new repository
jj git init

# Clone an existing repository
jj git clone <repository_url>

# Initialize in existing Git repository
jj git init --git-repo .
```

### Checking Status

```bash
# View current working copy status
jj st
jj status

# View with more detail
jj status --verbose
```

Example output:
```
Working copy : rlvkpyqy 2f6ff13a (empty) (no description set)
Parent commit: qpvuntsm 4cfa768e hello.md
```

### Viewing History

```bash
# View commit history
jj log

# View with graph
jj log --graph

# Limit number of commits
jj log -n 10

# View specific revisions
jj log -r <revset>
```

### Making Changes and Commits

```bash
# Create a new empty change
jj new

# Create new change with message
jj new -m "Add new feature"

# Add description to current change
jj describe
jj describe -m "Your commit message"

# Commit is automatically created from working copy changes
# when you run most jj commands
```

### Viewing Differences

```bash
# Show working copy changes
jj diff

# Show changes in specific commit
jj diff -r <commit>

# Show changes between commits
jj diff -r <commit1> -r <commit2>
```

## History Management

### Viewing and Navigation

```bash
# View operation history (like Git's reflog)
jj op log

# Undo last operation
jj undo

# Redo operation
jj op restore <operation_id>

# Show specific commit
jj show <commit>
```

### Modifying History

Jujutsu excels at history modification with automatic rebasing:

```bash
# Edit commit message
jj describe -r <commit> -m "New message"

# Modify commit interactively
jj diffedit -r <commit>

# Split a commit
jj split -r <commit>

# Squash changes into parent
jj squash

# Move changes between commits
jj squash -i
```

## Branch Management

Jujutsu has a unique approach to branching:

### Key Differences from Git

- **No "current branch" concept**: You work with commits directly
- **Bookmarks**: Named references to commits (similar to Git branches)
- **Commits can exist without branches**: Unlike Git, commits don't need to be on a named branch

### Working with Bookmarks

```bash
# Create bookmark
jj bookmark create <name>

# Move bookmark to different commit
jj bookmark set <name> -r <commit>

# Delete bookmark
jj bookmark delete <name>

# List bookmarks
jj bookmark list
```

### Creating and Managing Changes

```bash
# Start new change from specific commit
jj new -r <commit>

# Create change with specific parent
jj new <parent_commit>

# Abandon a change
jj abandon <commit>
```

## Remote Operations

### Setting up Remotes

```bash
# Add remote
jj git remote add origin <url>

# List remotes
jj git remote list
```

### Fetching and Pushing

```bash
# Fetch from remote
jj git fetch

# Fetch from specific remote
jj git fetch origin

# Push bookmark to remote
jj git push --bookmark <bookmark_name>

# Push all bookmarks
jj git push --all
```

### Working with Remote Branches

```bash
# Track remote bookmark
jj bookmark track <remote_bookmark>@origin

# Create local bookmark from remote
jj bookmark create <local_name> -r <remote_bookmark>@origin
```

## Conflict Resolution

Jujutsu's conflict handling is one of its most distinctive features:

### Key Advantages

- **Conflicts are first-class**: Can be recorded in commits
- **No workflow interruption**: Commands don't fail due to conflicts
- **Flexible resolution**: Resolve conflicts when convenient
- **Automatic propagation**: Conflict resolution automatically rebases descendants

### Conflict Workflow

1. **Conflicts are automatically recorded** when they occur
2. **Continue working** - other operations aren't blocked
3. **Resolve when convenient** by editing conflict markers
4. **Automatic updates** propagate to descendant commits

### Conflict Markers

Jujutsu uses enhanced conflict markers:

```
<<<<<<< Conflict 1 of 1
%%%%%%% Changes from base to side #1
 apple
-grape
+grapefruit
 orange
+++++++ Contents of side #2
APPLE
GRAPE
ORANGE
>>>>>>> Conflict 1 of 1 ends
```

### Resolution Commands

```bash
# Show conflicts
jj resolve --list

# Resolve conflicts interactively
jj resolve

# Show conflict in specific file
jj resolve <file>
```

## Advanced Features

### Rebasing

Automatic rebasing is a core feature:

```bash
# Manual rebase
jj rebase -s <source> -d <destination>

# Rebase range of commits
jj rebase -s <start> -d <destination>

# Note: Descendants automatically rebase when parents are modified
```

### Moving Changes Between Commits

```bash
# Move specific changes to parent
jj squash <file>

# Interactive squashing
jj squash -i

# Move changes from commit to working copy
jj edit <commit>
```

### Splitting and Combining

```bash
# Split current change interactively
jj split

# Split specific commit
jj split -r <commit>

# Combine changes from multiple commits
jj fold <commit1> <commit2>
```

## Common Workflows

### Feature Development

```bash
# 1. Start new feature
jj new -m "Start feature X"

# 2. Make changes (automatically committed to working copy)
# Edit files...

# 3. Create commits as you go
jj describe -m "Implement core feature"
jj new -m "Add tests"

# 4. Continue development
# More file edits...

# 5. Clean up history before sharing
jj squash  # Combine related changes
jj split   # Split large commits

# 6. Push when ready
jj bookmark create feature-x
jj git push --bookmark feature-x
```

### Bug Fix Workflow

```bash
# 1. Start from problematic commit
jj new -r <bug_commit>

# 2. Make fix
# Edit files...
jj describe -m "Fix bug: detailed description"

# 3. Rebase onto latest
jj rebase -d main@origin

# 4. Push fix
jj bookmark create bugfix-123
jj git push --bookmark bugfix-123
```

### Code Review Workflow

```bash
# 1. Address review feedback
jj new -r <reviewed_commit>

# 2. Make changes
# Edit files based on feedback...

# 3. Squash changes into original commit
jj squash -r <reviewed_commit>

# 4. Update remote
jj git push --bookmark <feature_branch>
```

## Migration from Git

### Command Mapping

| Git Command | Jujutsu Equivalent | Notes |
|-------------|-------------------|-------|
| `git status` | `jj status` | Shows working copy state |
| `git log` | `jj log` | More powerful revsets |
| `git add` | (automatic) | Files tracked automatically |
| `git commit` | `jj describe` | Working copy auto-committed |
| `git checkout` | `jj edit` | Change working copy |
| `git branch` | `jj bookmark create` | Different branching model |
| `git merge` | `jj merge` | Conflicts handled differently |
| `git rebase` | `jj rebase` | Automatic descendant rebasing |
| `git stash` | (not needed) | Working copy always clean |

### Key Mindset Shifts

1. **No staging area**: Think in terms of commits, not staging
2. **Working copy is a commit**: Changes are automatically tracked
3. **Conflicts are not blocking**: Continue working while conflicts exist
4. **History is malleable**: Freely modify commits with automatic rebasing
5. **Bookmarks vs branches**: Commits can exist without named references

### Common Git Workflows in Jujutsu

**Git's `git add -p; git commit`** becomes:
```bash
jj split  # Interactively choose what to commit
```

**Git's `git commit --amend`** becomes:
```bash
# Changes automatically amend working copy commit
jj describe -m "Updated message"
```

**Git's interactive rebase** becomes:
```bash
jj rebase -s <start> -d <destination>
# Descendants automatically rebase
```

## Configuration

### Basic Configuration

```bash
# Set user information
jj config set --user user.name "Your Name"
jj config set --user user.email "your.email@example.com"

# Set default editor
jj config set --user ui.editor "code --wait"
```

### Working Copy Configuration

```bash
# Control automatic file tracking
jj config set --user snapshot.auto-track false

# Configure conflict marker style
jj config set --user ui.conflict-marker-style diff
```

### Repository-specific Configuration

Create `.jj/config.toml` in your repository:

```toml
[user]
name = "Your Name"
email = "your.email@example.com"

[ui]
pager = "less -FRX"

[snapshot]
auto-track = true
```

---

## Getting Help

```bash
# General help
jj help

# Command-specific help
jj help <command>

# List all commands
jj help --help
```

## Resources

- **Official Repository**: https://github.com/martinvonz/jj
- **Documentation**: https://martinvonz.github.io/jj/
- **Tutorial**: Run through `jj tutorial` command
- **Community**: GitHub Discussions and Issues

---

*Note: This documentation is based on official Jujutsu documentation as of August 2025. Jujutsu is experimental software, so commands and behaviors may change. Always refer to `jj help` for the most current information.*