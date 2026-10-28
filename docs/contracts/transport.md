# Transport and retries

`HttpTransport::post` issues one cancellable JSON POST and reads one bounded
response. Reqwest protocol retries, redirects, and environment proxy discovery
are disabled. Protocol paths append to validated base URL prefixes. HTTPS uses
Rustls; explicit local HTTP uses the same transport. Building the native TLS
dependency requires a C compiler (Xcode Command Line Tools on macOS, a system C
toolchain on Linux). No subscriber is installed by the library.

The attempt timeout covers connection, headers, and body consumption. Cancellation
drops the in-flight future. The default timeout is 120 seconds and response bound
is 16 MiB. Declared and streamed bodies both respect that bound. Truncated bodies,
malformed JSON, connection failures, timeouts, and cancellation are typed errors.
Status errors distinguish authentication, rate limits, context capacity,
unsupported capabilities, contract failures, and other HTTP statuses. Recognized
native error codes can refine HTTP 400/422 classification; unknown server text is
never exposed as a diagnostic. Valid model refusals belong to provider outcomes,
not transport retries.

`RetryPolicy` is a pure decision component: given the completed attempt number,
typed failure, and caller-supplied current time, it returns the next permitted
delay or none. Defaults allow three total attempts, starting at one second and
doubling, capped at 30 seconds. Retryable failures are connection failures,
timeouts, HTTP 429, and HTTP 500/502/503/504/524/529. Retry-After supports seconds and HTTP
dates, is capped, and never shortens normal backoff. Invalid hints are ignored.
Authentication, schema/capability/capacity failures, cancellations, and refusals
are not retried by this policy. Explicit model changes never occur here.

The evaluator owns the loop, clock/delay scheduling, and attempt records. Adapters
do not retry. Transport has no ledger dependency and holds no storage transaction.
Diagnostics emit status and byte count only. Headers containing credentials are
marked sensitive, responses redact bodies/headers in Debug, and normalized errors
retain no raw request/response contents or key values. Loopback tests use owned
fixture server tasks and paused time rather than hosted services or retry sleeps.
