# Repository Guidelines

## Project Structure & Module Organization

This repository is currently an empty Git workspace: no application code, build configuration, tests, or assets have been committed. Establish the layout when adding the first implementation, and update this guide to match it.

Suggested directories:

- `src/`: application source, grouped by responsibility.
- `tests/`: automated tests and fixtures.
- `assets/`: icons and other static resources, if needed.
- `docs/`: architecture notes and development instructions.

Keep generated output and local environment files out of version control.

## Build, Test, and Development Commands

No build, test, or local run commands are configured yet. When introducing tooling, document the exact installation, build, test, and run commands here and in a project README. Prefer a single documented entry point for each operation.

Useful repository checks today:

- `git status --short`: inspect pending changes.
- `git diff --check`: detect whitespace errors in tracked changes.
- `git diff`: review unstaged changes before committing.

## Coding Style & Naming Conventions

No language or formatter has been selected. Use the chosen language's standard formatter and linter, and commit their configuration alongside the first source files. Keep indentation consistent within each file, use descriptive names, and follow the language's established naming conventions.

## Testing Guidelines

No test framework or coverage threshold exists yet. Add tests with new behavior, including relevant failure cases. Use descriptive test names that identify the behavior being checked. Document the test runner and any required services when the first tests are introduced.

## Commit & Pull Request Guidelines

There is no commit history from which to infer conventions. Until a convention is established, use concise, imperative commit subjects such as `Add initial widget implementation`. Keep commits focused.

Pull requests should explain the change, its purpose, and validation performed. Link relevant issues and include screenshots for visible UI changes. Explicitly identify checks that could not be run.
