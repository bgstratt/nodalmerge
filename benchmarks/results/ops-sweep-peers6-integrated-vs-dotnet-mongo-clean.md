## NodalMerge Performance Sweep (Peers 6, Mongo Backend, iterations=15)

### MAP Scenario
| Ops | Rust Integrated avg ms | Rust Integrated ops/s | Dotnet Mongo avg ms | Dotnet Mongo ops/s |
| --- | --- | --- | --- | --- |
| 6 | 96.36 | 373.6 | 97.38 | 369.69 |
| 12 | 149.48 | 481.67 | 145.83 | 493.73 |
| 30 | 336.11 | 535.54 | 315.33 | 570.83 |
### LIST Scenario
| Ops | Rust Integrated avg ms | Rust Integrated ops/s | Dotnet Mongo avg ms | Dotnet Mongo ops/s |
| --- | --- | --- | --- | --- |
| 6 | 115.72 | 311.1 | 108.24 | 332.59 |
| 12 | 206.9 | 347.99 | 172.13 | 418.29 |
| 30 | 450.89 | 399.21 | 375.74 | 479.05 |
### BLOB Scenario
| Ops | Rust Integrated avg ms | Rust Integrated ops/s | Dotnet Mongo avg ms | Dotnet Mongo ops/s |
| --- | --- | --- | --- | --- |
| 6 | 99.14 | 363.12 | 86.86 | 414.46 |
| 12 | 146.46 | 491.6 | 153.27 | 469.76 |
| 30 | 360.35 | 499.51 | 325.35 | 553.25 |

