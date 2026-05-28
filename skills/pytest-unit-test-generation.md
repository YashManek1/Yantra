---
name: pytest-unit-test-generation
version: 1
applies_to:
  language: python
  task_class: any
---

## Pytest Unit Test Generation

Conventions for generating pytest-based unit tests in the Yantra eval harness.

### Rules
- One test file per module under `tests/`.
- Use `pytest.fixture` for shared setup; avoid global state.
- Name tests `test_<what>_<condition>_<expected>`.
- Mock only at system boundaries (HTTP, filesystem, subprocess).

### Pattern: parameterized correctness test
```python
import pytest

@pytest.mark.parametrize("input_val, expected", [
    (0, "zero"),
    (1, "one"),
    (-1, "negative"),
])
def test_classify_number_returns_correct_label(input_val, expected):
    assert classify(input_val) == expected
```
