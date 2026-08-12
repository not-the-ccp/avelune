# Production backend testing strategy

Avelune treats tests as evidence about meaningful codec properties, not as a line-counting exercise. A test should normally protect a format invariant, a cross-implementation contract, a state-machine property, a resource boundary, or a measured performance/rate-quality property.

## Evidence layers

1. **Normative unit and differential tests** cover syntax, reconstruction, resource limits, and exact agreement with the independent Draft Generation 1 decoders.
2. **Property tests** generate many valid values and frames with shrinking, targeting canonical varints, entropy round trips, lossless video/audio, CPU dispatch, and thread-policy invariance.
3. **Metamorphic tests** check properties that should survive representation changes: fragmentation, reserve/commit versus copy, epoch reset, and lossless decode/re-encode.
4. **Stable mutation tests** run on every PR without nightly tooling and deliberately perturb valid codec/container seeds while requiring deterministic, panic-free classification.
5. **Coverage-guided fuzzing** runs on a schedule with separate entropy, ALV1, ALA1, container-streaming, and structured-valid-video targets. Structured generation is important because random malformed bytes otherwise spend most fuzz time in early rejection paths.
6. **Stateful soak/concurrency tests** exercise reference eviction, allocation reuse, epoch reset, and independent decoder instances for hundreds or thousands of operations.
7. **Sanitizers** instrument both the safe production engine and the isolated unsafe kernel crate on scheduled CI.
8. **E2E integration gates** exercise CLI, container streaming, scalar/SIMD WASM, HTTP Range behavior, and actual Chromium.
9. **Deterministic codec regression tests** build PR base and head on the same runner and encode the same generated media. Size and quality receive tight, tradeoff-aware checks; timing only has broad catastrophic-regression thresholds because hosted-runner timing is noisy.
10. **Scheduled benchmark history** records machine-readable JSON/CSV at multiple quantizers plus the production lab. The repository intentionally does not include graphing/UI; consumers can plot the artifacts later.
11. **Heterogeneous external media** is an additional evidence tier, not a definition of the real world. Generated edge cases, adversarial inputs, fuzzing, and normative tests remain independent acceptance constraints.

## PR versus scheduled CI

PR CI must remain fast enough that developers do not routinely skip it. Stable Rust property/metamorphic/mutation tests and a small deterministic media comparison therefore run on PRs. Nightly fuzzing, sanitizers, long soak tests, and repeated benchmark history are scheduled/manual jobs.

## Performance regression policy

Deterministic properties such as output size and decoded quality can be compared tightly. Wall-clock measurements are noisy on shared CI hardware, so PR timing gates only reject large same-runner regressions. Raw samples and medians are retained in artifacts so scheduled or dedicated hardware can apply stricter statistical analysis later.

A size increase is not automatically a failure if it buys a meaningful quality gain, and a quality loss is not automatically a failure if it buys a meaningful rate reduction. CI records both instead of optimizing a single number.

## Coverage is diagnostic

Coverage reports may be generated to find unexercised code, especially in parsing/error paths and unsafe kernels. A high coverage percentage is not an acceptance criterion and should not motivate trivial tests.

## Platform honesty

A configured CI job is not a locally executed pass. AArch64, Windows/macOS, i686, ASan/TSan, and scheduled cargo-fuzz results must be reported separately from locally executed x86-64/WASM/Chromium validation.
