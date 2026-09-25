# Every query in the fresh comparison

[Report and methodology](../../TGREP_COMPARISON.md). Speedup >1 favors zgrep; intervals are descriptive 95% bootstrap intervals.

## CPython custom / local

[Raw samples](cpython-local.json)

| Query | Pattern | Flags | tgrep ms | zgrep ms | Speedup [95% interval] |
| --- | --- | --- | ---: | ---: | ---: |
| literal-rare | `PyUnicode_DecodeUTF8Stateful` | `-t c -t py -F` | 4.008 | 3.811 | 1.051 [1.017, 1.060] |
| literal-common | `PyObject` | `-t c -t py -F` | 25.897 | 26.172 | 0.989 [0.984, 0.996] |
| literal-absent | `PyUnicode_ZgrepMissing999` | `-t c -t py -F` | 3.337 | 3.181 | 1.049 [1.025, 1.079] |
| literal-long | `PyObject_GetOptionalAttrString` | `-t c -t py -F` | 4.627 | 4.373 | 1.058 [1.029, 1.092] |
| short | `if` | `-t c -t py -F` | 68.997 | 69.535 | 0.992 [0.984, 0.999] |
| broad | `return` | `-t c -t py -F` | 55.749 | 55.921 | 0.997 [0.989, 1.001] |
| alternation | `(PyLong\|PyFloat).*FromString` | `-t c -t py` | 9.641 | 9.021 | 1.069 [1.044, 1.086] |
| nested-alternation | `(PyObject\|PyType).*(GetAttr\|SetAttr)` | `-t c -t py` | 43.209 | 14.917 | 2.897 [2.852, 2.939] |
| regex-selective | `PyUnicode_[A-Za-z]+UTF8` | `-t c -t py` | 8.537 | 8.616 | 0.991 [0.944, 1.018] |
| regex-common | `def test_.*error` | `-t c -t py` | 19.132 | 19.306 | 0.991 [0.974, 1.009] |
| case-insensitive | `memoryerror` | `-t c -t py -i` | 7.853 | 7.877 | 0.997 [0.981, 1.017] |
| scoped | `PyObject_Call` | `-t c -t py -F -g Python/**` | 5.462 | 5.570 | 0.981 [0.959, 1.003] |

## CPython custom / server

[Raw samples](cpython-server.json)

| Query | Pattern | Flags | tgrep ms | zgrep ms | Speedup [95% interval] |
| --- | --- | --- | ---: | ---: | ---: |
| literal-rare | `PyUnicode_DecodeUTF8Stateful` | `-t c -t py -F` | 2.879 | 2.667 | 1.079 [0.980, 1.117] |
| literal-common | `PyObject` | `-t c -t py -F` | 7.658 | 7.380 | 1.038 [0.930, 1.088] |
| literal-absent | `PyUnicode_ZgrepMissing999` | `-t c -t py -F` | 2.276 | 2.166 | 1.050 [0.993, 1.101] |
| literal-long | `PyObject_GetOptionalAttrString` | `-t c -t py -F` | 3.748 | 3.366 | 1.113 [1.064, 1.192] |
| short | `if` | `-t c -t py -F` | 16.332 | 16.729 | 0.976 [0.924, 1.021] |
| broad | `return` | `-t c -t py -F` | 14.390 | 14.127 | 1.019 [0.990, 1.060] |
| alternation | `(PyLong\|PyFloat).*FromString` | `-t c -t py` | 4.488 | 4.171 | 1.076 [1.029, 1.158] |
| nested-alternation | `(PyObject\|PyType).*(GetAttr\|SetAttr)` | `-t c -t py` | 10.182 | 4.926 | 2.067 [1.988, 2.206] |
| regex-selective | `PyUnicode_[A-Za-z]+UTF8` | `-t c -t py` | 4.393 | 4.498 | 0.977 [0.905, 1.089] |
| regex-common | `def test_.*error` | `-t c -t py` | 6.474 | 6.238 | 1.038 [0.983, 1.064] |
| case-insensitive | `memoryerror` | `-t c -t py -i` | 4.144 | 4.343 | 0.954 [0.895, 1.008] |
| scoped | `PyObject_Call` | `-t c -t py -F -g Python/**` | 3.450 | 3.523 | 0.979 [0.945, 1.057] |

## Rust custom / local

[Raw samples](rust-local.json)

| Query | Pattern | Flags | tgrep ms | zgrep ms | Speedup [95% interval] |
| --- | --- | --- | ---: | ---: | ---: |
| literal-rare | `TypeVisitableExt` | `-F -t rust` | 21.942 | 21.917 | 1.001 [0.984, 1.022] |
| literal-common | `Result` | `-F -t rust` | 63.154 | 63.221 | 0.999 [0.993, 1.013] |
| literal-absent | `TypeVisitableZgrepMissing999` | `-F -t rust` | 16.864 | 16.231 | 1.039 [1.031, 1.063] |
| literal-long | `rustc_middle::ty::TyCtxt` | `-F -t rust` | 20.068 | 19.631 | 1.022 [1.001, 1.046] |
| short | `fn` | `-F -t rust` | 320.393 | 321.576 | 0.996 [0.988, 1.002] |
| broad | `impl` | `-F -t rust` | 157.688 | 159.617 | 0.988 [0.984, 0.994] |
| alternation | `(TyCtxt\|InferCtxt).*<'tcx>` | `-t rust` | 35.067 | 30.099 | 1.165 [1.154, 1.179] |
| nested-alternation | `(struct\|enum).*(TypeError\|RegionError)` | `-t rust` | 255.406 | 19.358 | 13.194 [13.111, 13.437] |
| regex-selective | `impl.*TypeVisitable` | `-t rust` | 21.605 | 21.415 | 1.009 [0.992, 1.029] |
| regex-common | `fn test_.*parse` | `-t rust` | 21.233 | 20.710 | 1.025 [0.990, 1.037] |
| case-insensitive | `borrowchecker` | `-i -t rust` | 17.258 | 17.163 | 1.006 [0.989, 1.033] |
| scoped | `TyCtxt` | `-F -t rust -g compiler/**` | 27.671 | 27.830 | 0.994 [0.976, 1.005] |

## Rust custom / server

[Raw samples](rust-server.json)

| Query | Pattern | Flags | tgrep ms | zgrep ms | Speedup [95% interval] |
| --- | --- | --- | ---: | ---: | ---: |
| literal-rare | `TypeVisitableExt` | `-F -t rust` | 5.716 | 5.711 | 1.001 [0.736, 1.305] |
| literal-common | `Result` | `-F -t rust` | 21.101 | 20.670 | 1.021 [0.943, 1.098] |
| literal-absent | `TypeVisitableZgrepMissing999` | `-F -t rust` | 2.799 | 2.253 | 1.242 [1.140, 1.666] |
| literal-long | `rustc_middle::ty::TyCtxt` | `-F -t rust` | 5.699 | 5.247 | 1.086 [0.994, 1.561] |
| short | `fn` | `-F -t rust` | 157.502 | 159.883 | 0.985 [0.979, 1.025] |
| broad | `impl` | `-F -t rust` | 70.174 | 67.541 | 1.039 [1.012, 1.062] |
| alternation | `(TyCtxt\|InferCtxt).*<'tcx>` | `-t rust` | 10.148 | 7.658 | 1.325 [1.095, 1.502] |
| nested-alternation | `(struct\|enum).*(TypeError\|RegionError)` | `-t rust` | 56.487 | 5.959 | 9.479 [7.760, 11.033] |
| regex-selective | `impl.*TypeVisitable` | `-t rust` | 5.632 | 5.993 | 0.940 [0.783, 1.104] |
| regex-common | `fn test_.*parse` | `-t rust` | 5.948 | 5.653 | 1.052 [0.899, 1.179] |
| case-insensitive | `borrowchecker` | `-i -t rust` | 3.749 | 3.417 | 1.097 [0.983, 1.352] |
| scoped | `TyCtxt` | `-F -t rust -g compiler/**` | 9.187 | 8.103 | 1.134 [0.960, 1.279] |

## 102 upstream queries / local

[Raw samples](upstream-rust-local.json)

| Query | Pattern | Flags | tgrep ms | zgrep ms | Speedup [95% interval] |
| --- | --- | --- | ---: | ---: | ---: |
| upstream-000 | `fn main` | `-t rust -F` | 174.717 | 172.750 | 1.011 [0.996, 1.023] |
| upstream-001 | `pub fn` | `-t rust -F` | 95.278 | 95.155 | 1.001 [0.990, 1.017] |
| upstream-002 | `pub struct` | `-t rust -F` | 63.632 | 63.652 | 1.000 [0.986, 1.019] |
| upstream-003 | `pub enum` | `-t rust -F` | 37.216 | 36.792 | 1.012 [0.991, 1.026] |
| upstream-004 | `pub trait` | `-t rust -F` | 38.910 | 38.555 | 1.009 [0.990, 1.020] |
| upstream-005 | `impl Display` | `-t rust -F` | 18.595 | 18.117 | 1.026 [0.993, 1.046] |
| upstream-006 | `impl Debug` | `-t rust -F` | 20.840 | 20.286 | 1.027 [0.979, 1.057] |
| upstream-007 | `impl From` | `-t rust -F` | 24.300 | 24.184 | 1.005 [0.983, 1.030] |
| upstream-008 | `impl Iterator` | `-t rust -F` | 28.665 | 28.596 | 1.002 [0.990, 1.015] |
| upstream-009 | `impl Drop` | `-t rust -F` | 23.508 | 23.440 | 1.003 [0.978, 1.024] |
| upstream-010 | `derive(Debug` | `-t rust -F` | 39.698 | 40.230 | 0.987 [0.973, 1.010] |
| upstream-011 | `derive(Clone` | `-t rust -F` | 35.939 | 35.785 | 1.004 [0.986, 1.022] |
| upstream-012 | `derive(Serialize` | `-t rust -F` | 17.779 | 17.316 | 1.027 [0.997, 1.047] |
| upstream-013 | `#[cfg(test)]` | `-t rust -F` | 34.437 | 34.598 | 0.995 [0.983, 1.016] |
| upstream-014 | `#[test]` | `-t rust -F` | 33.378 | 33.373 | 1.000 [0.948, 1.022] |
| upstream-015 | `#[inline]` | `-t rust -F` | 43.232 | 43.022 | 1.005 [0.997, 1.017] |
| upstream-016 | `#[must_use]` | `-t rust -F` | 23.733 | 23.696 | 1.002 [0.964, 1.022] |
| upstream-017 | `#[allow(` | `-t rust -F` | 42.639 | 42.340 | 1.007 [0.984, 1.023] |
| upstream-018 | `#[deny(` | `-t rust -F` | 19.177 | 18.287 | 1.049 [1.019, 1.060] |
| upstream-019 | `#[deprecated` | `-t rust -F` | 20.319 | 19.964 | 1.018 [0.994, 1.046] |
| upstream-020 | `unsafe fn` | `-t rust -F` | 39.799 | 39.458 | 1.009 [0.983, 1.025] |
| upstream-021 | `unsafe impl` | `-t rust -F` | 24.037 | 23.835 | 1.008 [0.993, 1.042] |
| upstream-022 | `unsafe {` | `-t rust -F` | 59.542 | 58.997 | 1.009 [0.994, 1.020] |
| upstream-023 | `Box<dyn` | `-t rust -F` | 26.569 | 25.921 | 1.025 [0.987, 1.044] |
| upstream-024 | `Arc<Mutex` | `-t rust -F` | 17.132 | 17.009 | 1.007 [0.970, 1.034] |
| upstream-025 | `Rc<RefCell` | `-t rust -F` | 17.022 | 17.213 | 0.989 [0.951, 1.033] |
| upstream-026 | `Vec<` | `-t rust -F` | 61.427 | 61.976 | 0.991 [0.976, 1.010] |
| upstream-027 | `HashMap<` | `-t rust -F` | 25.393 | 25.262 | 1.005 [0.978, 1.012] |
| upstream-028 | `HashSet<` | `-t rust -F` | 23.980 | 23.968 | 1.000 [0.984, 1.031] |
| upstream-029 | `BTreeMap<` | `-t rust -F` | 18.554 | 18.328 | 1.012 [0.985, 1.035] |
| upstream-030 | `Option<` | `-t rust -F` | 74.754 | 75.274 | 0.993 [0.973, 1.007] |
| upstream-031 | `Result<` | `-t rust -F` | 49.782 | 50.547 | 0.985 [0.968, 0.991] |
| upstream-032 | `Some(` | `-t rust -F` | 88.052 | 88.526 | 0.995 [0.982, 1.012] |
| upstream-033 | `None` | `-t rust -F` | 78.182 | 78.375 | 0.998 [0.983, 1.027] |
| upstream-034 | `Ok(` | `-t rust -F` | 54.826 | 55.438 | 0.989 [0.966, 1.008] |
| upstream-035 | `Err(` | `-t rust -F` | 45.036 | 45.072 | 0.999 [0.978, 1.011] |
| upstream-036 | `unwrap()` | `-t rust -F` | 62.362 | 61.781 | 1.009 [0.986, 1.019] |
| upstream-037 | `expect(` | `-t rust -F` | 49.587 | 48.454 | 1.023 [1.009, 1.042] |
| upstream-038 | `unwrap_or` | `-t rust -F` | 43.846 | 43.385 | 1.011 [0.994, 1.029] |
| upstream-039 | `match ` | `-t rust -F` | 90.970 | 91.191 | 0.998 [0.991, 1.012] |
| upstream-040 | `if let Some` | `-t rust -F` | 57.271 | 57.412 | 0.998 [0.981, 1.022] |
| upstream-041 | `if let Ok` | `-t rust -F` | 27.472 | 26.416 | 1.040 [1.017, 1.054] |
| upstream-042 | `while let` | `-t rust -F` | 33.027 | 33.152 | 0.996 [0.970, 1.012] |
| upstream-043 | `loop {` | `-t rust -F` | 34.480 | 34.676 | 0.994 [0.970, 1.018] |
| upstream-044 | `break;` | `-t rust -F` | 28.260 | 28.370 | 0.996 [0.981, 1.009] |
| upstream-045 | `continue;` | `-t rust -F` | 32.024 | 31.620 | 1.013 [0.994, 1.028] |
| upstream-046 | `return Err` | `-t rust -F` | 28.120 | 28.064 | 1.002 [0.988, 1.022] |
| upstream-047 | `return Ok` | `-t rust -F` | 25.964 | 25.674 | 1.011 [1.004, 1.018] |
| upstream-048 | `panic!(` | `-t rust -F` | 41.302 | 41.287 | 1.000 [0.988, 1.020] |
| upstream-049 | `todo!()` | `-t rust -F` | 21.547 | 21.704 | 0.993 [0.982, 1.011] |
| upstream-050 | `unimplemented!()` | `-t rust -F` | 21.055 | 20.802 | 1.012 [0.969, 1.052] |
| upstream-051 | `unreachable!()` | `-t rust -F` | 29.938 | 29.915 | 1.001 [0.958, 1.019] |
| upstream-052 | `assert!(` | `-t rust -F` | 55.626 | 55.470 | 1.003 [0.986, 1.013] |
| upstream-053 | `assert_eq!(` | `-t rust -F` | 64.199 | 64.190 | 1.000 [0.987, 1.014] |
| upstream-054 | `debug_assert!(` | `-t rust -F` | 26.188 | 25.304 | 1.035 [1.015, 1.047] |
| upstream-055 | `println!(` | `-t rust -F` | 45.398 | 45.466 | 0.999 [0.983, 1.013] |
| upstream-056 | `eprintln!(` | `-t rust -F` | 21.549 | 21.239 | 1.015 [0.982, 1.026] |
| upstream-057 | `format!(` | `-t rust -F` | 49.029 | 49.575 | 0.989 [0.981, 1.007] |
| upstream-058 | `write!(` | `-t rust -F` | 26.854 | 26.857 | 1.000 [0.972, 1.036] |
| upstream-059 | `writeln!(` | `-t rust -F` | 19.558 | 19.721 | 0.992 [0.967, 1.002] |
| upstream-060 | `vec!` | `-t rust -F` | 47.806 | 48.323 | 0.989 [0.974, 1.024] |
| upstream-061 | `macro_rules!` | `-t rust -F` | 45.872 | 45.858 | 1.000 [0.990, 1.008] |
| upstream-062 | `use std::` | `-t rust -F` | 93.430 | 94.215 | 0.992 [0.985, 1.011] |
| upstream-063 | `use crate::` | `-t rust -F` | 74.772 | 75.502 | 0.990 [0.978, 1.010] |
| upstream-064 | `use super::` | `-t rust -F` | 46.330 | 46.001 | 1.007 [0.988, 1.035] |
| upstream-065 | `mod tests` | `-t rust -F` | 31.804 | 31.750 | 1.002 [0.988, 1.015] |
| upstream-066 | `pub mod` | `-t rust -F` | 29.380 | 29.483 | 0.997 [0.966, 1.018] |
| upstream-067 | `extern crate` | `-t rust -F` | 41.957 | 42.411 | 0.989 [0.972, 1.009] |
| upstream-068 | `lifetime` | `-t rust -F` | 45.793 | 45.516 | 1.006 [0.997, 1.017] |
| upstream-069 | `where ` | `-t rust -F` | 64.385 | 63.684 | 1.011 [0.994, 1.032] |
| upstream-070 | `'static` | `-t rust -F` | 52.630 | 52.863 | 0.996 [0.972, 1.000] |
| upstream-071 | `dyn Fn(` | `-t rust -F` | 21.215 | 20.807 | 1.020 [0.987, 1.032] |
| upstream-072 | `async fn` | `-t rust -F` | 25.765 | 25.556 | 1.008 [0.977, 1.031] |
| upstream-073 | `await` | `-t rust -F` | 25.040 | 24.905 | 1.005 [0.997, 1.034] |
| upstream-074 | `.map(` | `-t rust -F` | 57.772 | 57.652 | 1.002 [0.977, 1.009] |
| upstream-075 | `.filter(` | `-t rust -F` | 36.770 | 35.933 | 1.023 [0.997, 1.090] |
| upstream-076 | `.collect()` | `-t rust -F` | 40.912 | 41.304 | 0.991 [0.967, 1.009] |
| upstream-077 | `.iter()` | `-t rust -F` | 55.670 | 55.805 | 0.998 [0.987, 1.007] |
| upstream-078 | `.into_iter()` | `-t rust -F` | 40.429 | 40.404 | 1.001 [0.984, 1.012] |
| upstream-079 | `impl<'a` | `-t rust -F` | 39.772 | 40.381 | 0.985 [0.971, 1.002] |
| upstream-080 | `PhantomData` | `-t rust -F` | 25.973 | 26.055 | 0.997 [0.965, 1.014] |
| upstream-081 | `Pin<` | `-t rust -F` | 20.481 | 20.712 | 0.989 [0.965, 1.036] |
| upstream-082 | `MaybeUninit` | `-t rust -F` | 23.659 | 23.569 | 1.004 [0.978, 1.027] |
| upstream-083 | `ManuallyDrop` | `-t rust -F` | 20.351 | 20.165 | 1.009 [0.994, 1.034] |
| upstream-084 | `NonNull` | `-t rust -F` | 20.848 | 20.843 | 1.000 [0.978, 1.021] |
| upstream-085 | `transmute` | `-t rust -F` | 35.775 | 36.112 | 0.991 [0.973, 1.016] |
| upstream-086 | `TODO` | `-t rust -F` | 18.534 | 18.406 | 1.007 [0.997, 1.021] |
| upstream-087 | `FIXME` | `-t rust -F` | 51.862 | 52.180 | 0.994 [0.978, 1.010] |
| upstream-088 | `HACK` | `-t rust -F` | 19.147 | 19.251 | 0.995 [0.964, 1.009] |
| upstream-089 | `XXX` | `-t rust -F` | 17.196 | 17.503 | 0.982 [0.961, 1.021] |
| upstream-090 | `deprecated` | `-t rust -F` | 25.951 | 25.871 | 1.003 [0.989, 1.026] |
| upstream-091 | `^use std::` | `-t rust` | 117.772 | 117.415 | 1.003 [0.985, 1.019] |
| upstream-092 | `^pub fn\s+\w+` | `-t rust` | 115.929 | 116.534 | 0.995 [0.987, 1.013] |
| upstream-093 | `^\s*fn\s+\w+` | `-t rust` | 375.945 | 380.516 | 0.988 [0.973, 0.998] |
| upstream-094 | `struct\s+\w+<` | `-t rust` | 133.868 | 135.566 | 0.987 [0.972, 1.011] |
| upstream-095 | `enum\s+\w+\s*\{` | `-t rust` | 68.975 | 69.211 | 0.997 [0.971, 1.012] |
| upstream-096 | `trait\s+\w+` | `-t rust` | 105.631 | 107.366 | 0.984 [0.972, 1.003] |
| upstream-097 | `type\s+\w+\s*=` | `-t rust` | 135.975 | 137.468 | 0.989 [0.966, 0.995] |
| upstream-098 | `const\s+\w+:\s+` | `-t rust` | 109.538 | 110.928 | 0.987 [0.972, 0.996] |
| upstream-099 | `static\s+\w+:` | `-t rust` | 67.465 | 67.414 | 1.001 [0.986, 1.009] |
| upstream-100 | `impl\s+\w+\s+for` | `-t rust` | 118.007 | 119.522 | 0.987 [0.970, 1.005] |
| upstream-101 | `where\s+\w+:\s+` | `-t rust` | 68.393 | 69.196 | 0.988 [0.977, 1.005] |

## 102 upstream queries / server

[Raw samples](upstream-rust-server.json)

| Query | Pattern | Flags | tgrep ms | zgrep ms | Speedup [95% interval] |
| --- | --- | --- | ---: | ---: | ---: |
| upstream-000 | `fn main` | `-t rust -F` | 94.824 | 93.359 | 1.016 [0.989, 1.065] |
| upstream-001 | `pub fn` | `-t rust -F` | 38.991 | 37.964 | 1.027 [0.995, 1.054] |
| upstream-002 | `pub struct` | `-t rust -F` | 26.505 | 25.940 | 1.022 [0.948, 1.092] |
| upstream-003 | `pub enum` | `-t rust -F` | 13.406 | 12.839 | 1.044 [0.942, 1.110] |
| upstream-004 | `pub trait` | `-t rust -F` | 15.942 | 14.990 | 1.064 [0.996, 1.139] |
| upstream-005 | `impl Display` | `-t rust -F` | 5.037 | 5.401 | 0.932 [0.873, 1.068] |
| upstream-006 | `impl Debug` | `-t rust -F` | 6.359 | 5.877 | 1.082 [0.957, 1.176] |
| upstream-007 | `impl From` | `-t rust -F` | 7.316 | 6.836 | 1.070 [0.981, 1.142] |
| upstream-008 | `impl Iterator` | `-t rust -F` | 9.033 | 8.877 | 1.018 [0.966, 1.108] |
| upstream-009 | `impl Drop` | `-t rust -F` | 8.517 | 7.834 | 1.087 [0.992, 1.276] |
| upstream-010 | `derive(Debug` | `-t rust -F` | 13.966 | 13.916 | 1.004 [0.960, 1.087] |
| upstream-011 | `derive(Clone` | `-t rust -F` | 11.947 | 11.648 | 1.026 [0.944, 1.071] |
| upstream-012 | `derive(Serialize` | `-t rust -F` | 4.630 | 3.992 | 1.160 [1.049, 1.286] |
| upstream-013 | `#[cfg(test)]` | `-t rust -F` | 10.036 | 10.131 | 0.991 [0.937, 1.076] |
| upstream-014 | `#[test]` | `-t rust -F` | 11.068 | 11.471 | 0.965 [0.898, 1.058] |
| upstream-015 | `#[inline]` | `-t rust -F` | 12.335 | 11.266 | 1.095 [0.980, 1.120] |
| upstream-016 | `#[must_use]` | `-t rust -F` | 6.648 | 7.040 | 0.944 [0.862, 1.123] |
| upstream-017 | `#[allow(` | `-t rust -F` | 15.688 | 15.284 | 1.026 [0.993, 1.058] |
| upstream-018 | `#[deny(` | `-t rust -F` | 6.443 | 6.208 | 1.038 [0.874, 1.173] |
| upstream-019 | `#[deprecated` | `-t rust -F` | 6.105 | 6.270 | 0.974 [0.865, 1.080] |
| upstream-020 | `unsafe fn` | `-t rust -F` | 12.699 | 12.445 | 1.020 [0.963, 1.069] |
| upstream-021 | `unsafe impl` | `-t rust -F` | 7.660 | 7.374 | 1.039 [0.919, 1.158] |
| upstream-022 | `unsafe {` | `-t rust -F` | 22.509 | 22.177 | 1.015 [0.970, 1.046] |
| upstream-023 | `Box<dyn` | `-t rust -F` | 9.325 | 8.933 | 1.044 [0.877, 1.146] |
| upstream-024 | `Arc<Mutex` | `-t rust -F` | 3.948 | 4.099 | 0.963 [0.890, 1.139] |
| upstream-025 | `Rc<RefCell` | `-t rust -F` | 4.444 | 4.776 | 0.931 [0.843, 1.067] |
| upstream-026 | `Vec<` | `-t rust -F` | 20.725 | 20.879 | 0.993 [0.945, 1.027] |
| upstream-027 | `HashMap<` | `-t rust -F` | 7.443 | 7.459 | 0.998 [0.937, 1.113] |
| upstream-028 | `HashSet<` | `-t rust -F` | 6.539 | 6.712 | 0.974 [0.864, 1.098] |
| upstream-029 | `BTreeMap<` | `-t rust -F` | 5.336 | 5.247 | 1.017 [0.898, 1.099] |
| upstream-030 | `Option<` | `-t rust -F` | 26.488 | 25.852 | 1.025 [0.998, 1.049] |
| upstream-031 | `Result<` | `-t rust -F` | 17.439 | 17.574 | 0.992 [0.924, 1.105] |
| upstream-032 | `Some(` | `-t rust -F` | 31.276 | 30.996 | 1.009 [0.969, 1.043] |
| upstream-033 | `None` | `-t rust -F` | 27.159 | 26.048 | 1.043 [0.461, 1.082] |
| upstream-034 | `Ok(` | `-t rust -F` | 18.188 | 17.713 | 1.027 [0.934, 1.113] |
| upstream-035 | `Err(` | `-t rust -F` | 13.979 | 13.988 | 0.999 [0.949, 1.081] |
| upstream-036 | `unwrap()` | `-t rust -F` | 21.279 | 20.186 | 1.054 [0.994, 1.085] |
| upstream-037 | `expect(` | `-t rust -F` | 16.921 | 16.538 | 1.023 [0.953, 1.086] |
| upstream-038 | `unwrap_or` | `-t rust -F` | 13.991 | 12.995 | 1.077 [0.998, 1.131] |
| upstream-039 | `match ` | `-t rust -F` | 31.658 | 31.717 | 0.998 [0.984, 1.042] |
| upstream-040 | `if let Some` | `-t rust -F` | 18.588 | 18.660 | 0.996 [0.938, 1.051] |
| upstream-041 | `if let Ok` | `-t rust -F` | 8.393 | 6.927 | 1.212 [1.052, 1.301] |
| upstream-042 | `while let` | `-t rust -F` | 10.063 | 9.438 | 1.066 [0.961, 1.180] |
| upstream-043 | `loop {` | `-t rust -F` | 12.307 | 11.880 | 1.036 [0.915, 1.093] |
| upstream-044 | `break;` | `-t rust -F` | 7.917 | 8.055 | 0.983 [0.866, 1.056] |
| upstream-045 | `continue;` | `-t rust -F` | 9.506 | 9.172 | 1.036 [0.900, 1.156] |
| upstream-046 | `return Err` | `-t rust -F` | 8.107 | 8.078 | 1.004 [0.909, 1.089] |
| upstream-047 | `return Ok` | `-t rust -F` | 7.511 | 7.101 | 1.058 [0.916, 1.105] |
| upstream-048 | `panic!(` | `-t rust -F` | 15.343 | 14.993 | 1.023 [0.980, 1.129] |
| upstream-049 | `todo!()` | `-t rust -F` | 7.536 | 8.500 | 0.887 [0.825, 1.021] |
| upstream-050 | `unimplemented!()` | `-t rust -F` | 7.329 | 7.017 | 1.044 [1.005, 1.197] |
| upstream-051 | `unreachable!()` | `-t rust -F` | 9.188 | 8.524 | 1.078 [0.911, 1.162] |
| upstream-052 | `assert!(` | `-t rust -F` | 17.936 | 17.800 | 1.008 [0.955, 1.055] |
| upstream-053 | `assert_eq!(` | `-t rust -F` | 23.947 | 23.316 | 1.027 [0.989, 1.064] |
| upstream-054 | `debug_assert!(` | `-t rust -F` | 7.565 | 7.094 | 1.066 [0.967, 1.210] |
| upstream-055 | `println!(` | `-t rust -F` | 20.616 | 19.649 | 1.049 [1.015, 1.078] |
| upstream-056 | `eprintln!(` | `-t rust -F` | 6.872 | 6.907 | 0.995 [0.930, 1.057] |
| upstream-057 | `format!(` | `-t rust -F` | 16.865 | 16.526 | 1.021 [0.961, 1.196] |
| upstream-058 | `write!(` | `-t rust -F` | 7.970 | 7.663 | 1.040 [0.936, 1.115] |
| upstream-059 | `writeln!(` | `-t rust -F` | 6.053 | 5.458 | 1.109 [0.961, 1.218] |
| upstream-060 | `vec!` | `-t rust -F` | 16.179 | 16.012 | 1.010 [0.952, 1.119] |
| upstream-061 | `macro_rules!` | `-t rust -F` | 18.277 | 17.602 | 1.038 [0.977, 1.062] |
| upstream-062 | `use std::` | `-t rust -F` | 38.726 | 37.793 | 1.025 [0.986, 1.052] |
| upstream-063 | `use crate::` | `-t rust -F` | 26.325 | 25.568 | 1.030 [0.962, 1.084] |
| upstream-064 | `use super::` | `-t rust -F` | 16.565 | 15.326 | 1.081 [1.001, 1.134] |
| upstream-065 | `mod tests` | `-t rust -F` | 9.819 | 9.971 | 0.985 [0.923, 1.059] |
| upstream-066 | `pub mod` | `-t rust -F` | 10.799 | 10.628 | 1.016 [0.939, 1.085] |
| upstream-067 | `extern crate` | `-t rust -F` | 19.528 | 19.729 | 0.990 [0.946, 1.052] |
| upstream-068 | `lifetime` | `-t rust -F` | 16.935 | 17.020 | 0.995 [0.937, 1.081] |
| upstream-069 | `where ` | `-t rust -F` | 22.793 | 22.392 | 1.018 [0.946, 1.076] |
| upstream-070 | `'static` | `-t rust -F` | 20.049 | 20.077 | 0.999 [0.948, 1.086] |
| upstream-071 | `dyn Fn(` | `-t rust -F` | 7.515 | 7.309 | 1.028 [0.948, 1.122] |
| upstream-072 | `async fn` | `-t rust -F` | 8.978 | 9.137 | 0.983 [0.887, 1.103] |
| upstream-073 | `await` | `-t rust -F` | 7.801 | 8.021 | 0.973 [0.902, 1.041] |
| upstream-074 | `.map(` | `-t rust -F` | 18.079 | 18.834 | 0.960 [0.943, 1.123] |
| upstream-075 | `.filter(` | `-t rust -F` | 10.380 | 10.556 | 0.983 [0.896, 1.031] |
| upstream-076 | `.collect()` | `-t rust -F` | 13.329 | 13.344 | 0.999 [0.890, 1.089] |
| upstream-077 | `.iter()` | `-t rust -F` | 17.918 | 19.430 | 0.922 [0.855, 1.092] |
| upstream-078 | `.into_iter()` | `-t rust -F` | 13.302 | 12.435 | 1.070 [1.003, 1.132] |
| upstream-079 | `impl<'a` | `-t rust -F` | 15.088 | 14.908 | 1.012 [0.985, 1.081] |
| upstream-080 | `PhantomData` | `-t rust -F` | 9.876 | 9.784 | 1.009 [0.916, 1.087] |
| upstream-081 | `Pin<` | `-t rust -F` | 6.351 | 6.515 | 0.975 [0.838, 1.081] |
| upstream-082 | `MaybeUninit` | `-t rust -F` | 7.137 | 6.972 | 1.024 [0.968, 1.071] |
| upstream-083 | `ManuallyDrop` | `-t rust -F` | 6.078 | 5.212 | 1.166 [1.073, 1.270] |
| upstream-084 | `NonNull` | `-t rust -F` | 6.036 | 5.914 | 1.020 [0.937, 1.136] |
| upstream-085 | `transmute` | `-t rust -F` | 10.424 | 10.213 | 1.021 [0.948, 1.047] |
| upstream-086 | `TODO` | `-t rust -F` | 5.326 | 5.499 | 0.969 [0.889, 1.083] |
| upstream-087 | `FIXME` | `-t rust -F` | 17.192 | 16.929 | 1.016 [0.948, 1.058] |
| upstream-088 | `HACK` | `-t rust -F` | 5.311 | 5.132 | 1.035 [0.938, 1.145] |
| upstream-089 | `XXX` | `-t rust -F` | 4.989 | 5.120 | 0.974 [0.754, 1.012] |
| upstream-090 | `deprecated` | `-t rust -F` | 8.447 | 8.070 | 1.047 [0.945, 1.221] |
| upstream-091 | `^use std::` | `-t rust` | 38.821 | 38.091 | 1.019 [0.997, 1.065] |
| upstream-092 | `^pub fn\s+\w+` | `-t rust` | 43.970 | 42.546 | 1.033 [0.977, 1.075] |
| upstream-093 | `^\s*fn\s+\w+` | `-t rust` | 144.877 | 142.933 | 1.014 [0.992, 1.039] |
| upstream-094 | `struct\s+\w+<` | `-t rust` | 43.489 | 41.529 | 1.047 [0.985, 1.083] |
| upstream-095 | `enum\s+\w+\s*\{` | `-t rust` | 23.854 | 22.906 | 1.041 [1.005, 1.106] |
| upstream-096 | `trait\s+\w+` | `-t rust` | 44.484 | 44.885 | 0.991 [0.965, 1.036] |
| upstream-097 | `type\s+\w+\s*=` | `-t rust` | 41.808 | 41.197 | 1.015 [0.988, 1.041] |
| upstream-098 | `const\s+\w+:\s+` | `-t rust` | 36.114 | 35.003 | 1.032 [1.006, 1.057] |
| upstream-099 | `static\s+\w+:` | `-t rust` | 20.312 | 20.443 | 0.994 [0.944, 1.028] |
| upstream-100 | `impl\s+\w+\s+for` | `-t rust` | 38.506 | 38.186 | 1.008 [0.980, 1.068] |
| upstream-101 | `where\s+\w+:\s+` | `-t rust` | 18.272 | 18.959 | 0.964 [0.906, 1.016] |
