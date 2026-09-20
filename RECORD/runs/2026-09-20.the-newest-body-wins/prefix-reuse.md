# Prefix reuse, the fix against its parent commit

Turn 1 has no previous render to share a prefix with and is not in the
table. `before` is `2e87d2c`, `after` is the commit this run belongs to.

## fragments, the corpus as written

- session prefix reuse **93.95% -> 87.14%**, -6.81 points
- history bucket, summed over renders: **7200 -> 6432 tokens**

| turn | reuse before | reuse after | prompt before | prompt after |
| ---: | ---: | ---: | ---: | ---: |
| 2 | 79.7% | 79.7% | 733 | 733 |
| 3 | 86.7% | 86.7% | 845 | 845 |
| 4 | 91.6% | 91.6% | 922 | 922 |
| 5 | 97.3% | 97.3% | 948 | 948 |
| 6 | 90.5% | 47.7% | 1110 | 1033 |
| 7 | 97.9% | 97.8% | 1134 | 1056 |
| 8 | 97.8% | 97.5% | 1160 | 1083 |
| 9 | 98.3% | 98.2% | 1235 | 1158 |
| 10 | 88.7% | 88.1% | 1459 | 1382 |
| 11 | 98.2% | 98.2% | 1485 | 1407 |
| 12 | 93.6% | 63.1% | 1646 | 1492 |
| 13 | 98.2% | 98.0% | 1677 | 1523 |

## `--select-tokens 1024`

- session prefix reuse **93.61% -> 86.53%**, -7.08 points
- history bucket, summed over renders: **8331 -> 7455 tokens**

| turn | reuse before | reuse after | prompt before | prompt after |
| ---: | ---: | ---: | ---: | ---: |
| 2 | 69.5% | 69.5% | 874 | 874 |
| 3 | 95.9% | 95.9% | 911 | 911 |
| 4 | 92.2% | 92.2% | 988 | 988 |
| 5 | 97.4% | 97.4% | 1014 | 1014 |
| 6 | 89.2% | 44.4% | 1199 | 1110 |
| 7 | 98.0% | 97.9% | 1223 | 1134 |
| 8 | 97.9% | 97.7% | 1249 | 1161 |
| 9 | 97.2% | 97.0% | 1340 | 1251 |
| 10 | 88.0% | 87.3% | 1590 | 1501 |
| 11 | 98.5% | 98.3% | 1615 | 1527 |
| 12 | 92.8% | 61.3% | 1800 | 1624 |
| 13 | 98.3% | 98.1% | 1831 | 1655 |

