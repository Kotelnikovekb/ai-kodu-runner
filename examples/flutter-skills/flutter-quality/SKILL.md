---
name: flutter-quality
description: Apply the project's Flutter quality, testing, accessibility, and architecture practices.
---

Inspect the existing Flutter architecture before editing.

Prefer small, testable changes. Add or update unit/widget tests for behavior changes. Keep widgets accessible, preserve null-safety, avoid unnecessary rebuilds, and follow the project's existing state-management conventions.

Before finishing, run flutter analyze, dart format --output=none --set-exit-if-changed ., and flutter test.
