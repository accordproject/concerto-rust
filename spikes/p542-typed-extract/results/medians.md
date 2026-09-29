Native (glibc), ms: median of three round medians (60 samples each after 10 warm-up); the three rounds in brackets.

| stage | synthetic-large | conformance | concerto-core-test-data |
|---|---:|---:|---:|
| extract_total | 19.39 (18.16/19.39/19.91) | 3.45 (3.39/3.45/3.51) | 10.71 (10.05/10.71/12.48) |
| resolve | 5.36 (5.12/5.36/5.42) | 0.94 (0.91/0.94/0.95) | 3.48 (3.14/3.48/3.65) |
| result_build | 6.30 (5.87/6.30/6.38) | 1.57 (1.53/1.57/1.59) | 3.83 (3.60/3.83/4.06) |
| new_mm | 0.80 (0.74/0.80/0.86) | 0.10 (0.09/0.10/0.11) | 1.33 (1.23/1.33/1.33) |
| validate_only | 1.22 (1.14/1.22/1.26) | 0.49 (0.45/0.49/0.49) | 0.82 (0.76/0.82/0.98) |
| encode | 1.86 (1.86/1.86/1.93) | 0.27 (0.26/0.27/0.30) | 1.22 (1.19/1.22/1.33) |
| stage_clone | 1.85 (1.78/1.85/1.86) | 0.36 (0.33/0.36/0.37) | 1.14 (1.06/1.14/1.32) |
| drop | 3.89 (3.71/3.89/3.94) | 0.40 (0.33/0.40/0.45) | 2.23 (2.17/2.23/2.46) |
| walk_borrowed | 4.42 (4.22/4.42/4.59) | 0.96 (0.83/0.96/1.01) | 3.54 (3.14/3.54/3.93) |
| walk_typed | 4.17 (4.03/4.17/4.37) | 0.53 (0.52/0.53/0.56) | 3.15 (2.87/3.15/3.53) |
| walk_typed_direct | 0.90 (0.84/0.90/0.92) | 0.10 (0.10/0.10/0.10) | 0.58 (0.56/0.58/0.65) |
| cold_source_build_us | 8.58 (6.88/8.58/10.02) | 1.63 (1.60/1.63/1.63) | 5.69 (4.21/5.69/8.07) |

counts synthetic-large: {"real_commands":786,"borrowed":{"commands":786,"vocab":303},"typed":{"commands":786,"vocab":303},"typed_direct":{"commands":786,"vocab":303}}
counts conformance: {"real_commands":106,"borrowed":{"commands":106,"vocab":58},"typed":{"commands":106,"vocab":58},"typed_direct":{"commands":106,"vocab":58}}
counts concerto-core-test-data: {"real_commands":539,"borrowed":{"commands":539,"vocab":223},"typed":{"commands":539,"vocab":223},"typed_direct":{"commands":539,"vocab":223}}
