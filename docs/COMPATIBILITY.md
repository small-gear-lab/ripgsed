# Compatibility matrix

Full GNU sed compatibility is the minimum requirement of the project (D4 in [DECISIONS.md](DECISIONS.md)). The matrix in `compat/sed.tsv` lists what that means item by item and records how far each item has got.

## Where the items come from

- The structure of the GNU sed manual: invocation, options, exit status, script syntax, every command, the `s` command and its flags, addresses, regular expressions, escapes, locale behavior, the execution cycle, and the documented non-bugs.
- Only documentation was used. No source code was read or copied for the list.
- The manual is not complete. Behavior that is not documented is added when differential tests find it, with the source `tests`. An item missing from the matrix is not exempt from the requirement.

## Columns

| Column | Meaning |
| --- | --- |
| `id` | Stable identifier such as `CMD-005`. Never reused. |
| `area` | Group of the item (`command`, `option`, `address`, `regex`, ...). |
| `item` | The feature or behavior. |
| `status` | `unsupported`, `partial`, or `verified`. |
| `source` | Manual section the item comes from, or `tests`. |
| `notes` | Points that need care, edge cases, or the decision that affects it. |

## Status rules

- `unsupported`: no fixture passes. An unsupported construct is rejected with a compile-time error and is never approximated.
- `partial`: some of the fixtures covering the item pass and some do not.
- `verified`: every fixture covering the item passes. Nothing is `verified` by assertion: the status is computed from the test results.

The status column is rewritten by the test runner (`RIPGSED_UPDATE_MATRIX=1 cargo test`), not by hand. An item with no fixture keeps its status, and an item cannot be `verified` without fixtures. Fixtures covering a `verified` item are enforced by `cargo test`, so a verified item cannot silently regress.

## Summary

```sh
tools/compat-summary.sh
```

prints the number of items per area and status.
