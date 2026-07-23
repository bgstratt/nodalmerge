## NodalMerge Performance Sweep (Peers 6, Mongo Backend, iterations=15)

### MAP Scenario
| Ops | Rust Integrated avg ms | Rust Integrated ops/s | Dotnet Mongo avg ms | Dotnet Mongo ops/s |
| --- | --- | --- | --- | --- |
| 6 | 99.13 | 363.16 | 95.42 | 377.28 |
| 12 | 145.89 | 493.52 | 148.54 | 484.72 |
| 30 | 330.73 | 544.25 | 295.64 | 608.85 |
### LIST Scenario
| Ops | Rust Integrated avg ms | Rust Integrated ops/s | Dotnet Mongo avg ms | Dotnet Mongo ops/s |
| --- | --- | --- | --- | --- |
| 6 | 112.48 | 320.06 | 104.13 | 345.72 |
| 12 | 213.63 | 337.03 | 169.69 | 424.3 |
| 30 | 442.48 | 406.8 | 374.85 | 480.19 |
### BLOB Scenario
| Ops | Rust Integrated avg ms | Rust Integrated ops/s | Dotnet Mongo avg ms | Dotnet Mongo ops/s |
| --- | --- | --- | --- | --- |
| 6 | 97.01 | 371.1 | 86.93 | 414.13 |
| 12 | 150.84 | 477.33 | 151.99 | 473.72 |
| 30 | 353.35 | 509.41 | 326.02 | 552.11 |

