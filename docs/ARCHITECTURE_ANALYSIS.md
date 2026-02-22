# IronClaw Architecture: Deep Analysis

## What It Is

IronClaw is a self-hosted personal AI assistant built in Rust (~30K lines across 80+ files). It takes input from multiple channels (TUI, HTTP webhooks, Telegram/Slack via WASM, web browser), routes messages through an LLM-powered reasoning loop that calls tools, and runs tasks in parallel with Docker-based sandboxing. Core thesis: data stays local and encrypted, and the agent can expand its own capabilities by building new WASM tools at runtime.

---

## Glossary

| Term | Definition |
|------|-----------|
| **Aho-Corasick** | A string-searching algorithm that finds multiple patterns simultaneously in a single pass over the input. Used in IronClaw's sanitizer to detect injection patterns efficiently. |
| **AES-256-GCM** | Advanced Encryption Standard with 256-bit key and Galois/Counter Mode. An authenticated encryption scheme providing both confidentiality and integrity. Used for encrypting secrets at rest. |
| **AppArmor / SELinux** | Linux Mandatory Access Control (MAC) frameworks that enforce security policies beyond standard Unix permissions. Neither is configured in IronClaw's containers. |
| **Arc** | Atomically Reference Counted pointer in Rust. Enables safe shared ownership across threads by tracking how many references exist and freeing memory when the last one drops. |
| **async/await** | Rust's asynchronous programming model. Functions marked `async` return futures that are polled by a runtime (Tokio) rather than blocking OS threads. |
| **Axum** | A Rust web framework built on top of Tokio and Hyper. Used for IronClaw's web gateway and orchestrator HTTP API. |
| **BLAKE3** | A cryptographic hash function. Used in IronClaw to verify WASM binary integrity before loading. |
| **Bollard** | A Rust library for interacting with the Docker daemon API. Used to create, manage, and destroy containers programmatically. |
| **CAP_CHOWN** | A Linux capability that allows changing file ownership. Left enabled in IronClaw containers, creating a potential SUID escalation vector. |
| **CGNAT** | Carrier-Grade NAT. The IP range 100.64.0.0/10 used by ISPs for address sharing. Blocked in IronClaw's DNS rebinding defense. |
| **Circuit Breaker** | A resilience pattern that stops calling a failing service after N consecutive failures, waits a recovery period, then allows probe requests to test recovery. States: Closed (normal) then Open (blocking) then HalfOpen (probing). |
| **CONNECT tunnel** | An HTTP method that establishes a TCP tunnel through a proxy, typically used for HTTPS traffic. The proxy validates the target domain but cannot inspect encrypted traffic. |
| **Cranelift** | A code generator used by Wasmtime to compile WebAssembly to native machine code. |
| **DNS rebinding** | An attack where a domain's DNS record is changed between the allowlist check and the actual connection, causing the request to hit an internal IP instead of the expected external server. |
| **EMA** | Exponential Moving Average. A statistical method that gives more weight to recent observations. Used in IronClaw's estimation system to learn from actual job costs/durations. |
| **Epoch interruption** | A Wasmtime mechanism for interrupting running WASM code. A background thread periodically increments a counter; the WASM store is configured to trap when the counter reaches a deadline. |
| **Failover** | A resilience pattern where requests are routed to backup providers when the primary fails. IronClaw uses per-provider cooldown periods tracked with atomic counters. |
| **Fire-and-forget** | A pattern where a task is spawned without waiting for its result. Used in IronClaw for DB writes to avoid blocking the main loop, but creates unbounded task accumulation risk under load. |
| **FTS** | Full-Text Search. Text indexing that supports keyword queries. PostgreSQL uses tsvector/tsquery; libSQL uses FTS5 virtual tables. |
| **Fuel metering** | A Wasmtime mechanism that assigns a "fuel" budget to WASM execution. Each instruction consumes fuel; execution traps when fuel runs out. Prevents infinite loops and CPU abuse. |
| **Homoglyph** | A character that looks identical to another character but has a different Unicode codepoint. Example: Cyrillic "e" (U+0435) vs Latin "e" (U+0065). Used to bypass pattern matching. |
| **JoinSet** | A Tokio type that manages a set of spawned tasks and allows awaiting their completion. Used for parallel tool execution in the dispatcher. |
| **LRU** | Least Recently Used. A cache eviction policy that removes the entry that hasn't been accessed for the longest time. Used in IronClaw's LLM response cache. |
| **MAC** | Mandatory Access Control. A security model where the OS kernel enforces access policies that users cannot override (unlike standard Unix permissions). |
| **MCP** | Model Context Protocol. A standard for connecting LLMs to external tools and data sources. IronClaw implements an MCP client for consuming external tool servers. |
| **Mutex** | Mutual Exclusion lock. Only one task can hold the lock at a time. In Tokio, `Mutex` is async-aware and will not block the executor thread while waiting. |
| **Newtype** | A Rust pattern of wrapping a primitive type in a named struct (e.g., `struct UserId(u64)`) to add type safety and prevent accidental misuse. |
| **NFD/NFC** | Unicode Normalization Forms. NFD decomposes characters into base + combining marks; NFC composes them into single codepoints. Normalizing before pattern matching prevents homoglyph bypasses. |
| **Ordering::Relaxed** | The weakest memory ordering in Rust's atomic operations. Guarantees atomicity but not ordering relative to other operations. Sufficient when stale reads are acceptable (e.g., circuit breaker state). |
| **Reciprocal Rank Fusion (RRF)** | A method for combining results from multiple ranked lists. Formula: score(doc) = sum(1/(k + rank)) across all lists where the document appears. Higher k values favor top-ranked results. |
| **RFC1918** | The standard defining private IP address ranges: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16. Blocked in IronClaw's WASM HTTP requests to prevent SSRF. |
| **RwLock** | Read-Write Lock. Multiple readers can hold the lock simultaneously, but writers get exclusive access. Used for tool registry, session manager, and channel manager. |
| **Seccomp** | Secure Computing mode. A Linux kernel feature that restricts which system calls a process can make. Not configured in IronClaw's containers. |
| **select_all** | A futures combinator that merges multiple streams into one, yielding items from whichever stream produces next. Used to merge all channel message streams. |
| **Smart routing** | An LLM provider wrapper that classifies query complexity and routes simple queries to cheaper models. Tool calls always go to the primary model. |
| **SSRF** | Server-Side Request Forgery. An attack where a server is tricked into making requests to internal resources. Mitigated in IronClaw via DNS rebinding checks and domain allowlisting. |
| **SUID** | Set User ID. A Unix file permission bit that causes a program to run with the file owner's privileges rather than the caller's. Combined with CAP_CHOWN, enables privilege escalation. |
| **thiserror** | A Rust derive macro for implementing the standard Error trait. Generates boilerplate for error type definitions with format strings. |
| **TOCTOU** | Time-of-Check to Time-of-Use. A race condition where the state checked (e.g., "is this tool approved?") changes before the result is used (e.g., executing the tool). |
| **Tokio** | The async runtime used by IronClaw. It provides a multi-threaded executor for running async tasks, timers, I/O, and synchronization primitives (Mutex, RwLock, channels). |
| **WASI** | WebAssembly System Interface. A standard for WASM modules to interact with the host operating system. IronClaw uses WASI Preview 2 (component model). |
| **Wasmtime** | A standalone WebAssembly runtime. IronClaw uses it to compile and execute WASM tool modules with resource limits and capability-based security. |
| **WIT** | WebAssembly Interface Types. A language for describing the interfaces between WASM components and their hosts. IronClaw defines tool interfaces in wit/tool.wit. |
| **zeroize** | A Rust crate that overwrites memory with zeros when a value is dropped, preventing secrets from lingering in memory. Not currently used in IronClaw. |

---

## 1. Security Architecture (Defense in Depth)

Six independent layers, each catching what the others miss. The design principle is that no single layer is trusted -- each operates independently and redundantly.

### Layer 1: Safety Pipeline (src/safety/)

Every piece of external data passes through a 4-stage pipeline before reaching the LLM.

```
Tool Output
    |
    v
[1. Leak Detection] -- Scan for API keys, tokens, private keys
    |                  15 regex patterns, Aho-Corasick prefix filter
    |                  Actions: Block / Redact / Warn
    v
[2. Policy Check]   -- Enforce 7 default rules
    |                  Severity: Critical / High / Medium / Low
    |                  Actions: Block / Warn / Review / Sanitize
    v
[3. Sanitization]   -- Detect injection patterns
    |                  17 Aho-Corasick + 4 regex patterns
    |                  Escape content on Critical severity
    v
[4. XML Wrapping]   -- Structural boundary
    |                  <tool_output name="X" sanitized="true">
    |                  Escape XML entities (ampersand, angle brackets)
    v
LLM Context
```

#### Leak Detection Patterns (leak_detector.rs:406-522)

15 patterns organized by severity and action:

**Critical / Block (9 patterns):**
- `openai_api_key`: `sk-(?:proj-)?[a-zA-Z0-9]{20,}` -- OpenAI API keys
- `anthropic_api_key`: `sk-ant-api[a-zA-Z0-9_-]{90,}` -- Anthropic API keys
- `aws_access_key`: `AKIA[0-9A-Z]{16}` -- AWS access key IDs
- `github_token`: `gh[pousr]_[A-Za-z0-9_]{36,}` -- GitHub classic tokens
- `github_fine_grained_pat`: `github_pat_[a-zA-Z0-9]{22}_[a-zA-Z0-9]{59}`
- `stripe_api_key`: `sk_(?:live|test)_[a-zA-Z0-9]{24,}`
- `nearai_session`: `sess_[a-zA-Z0-9]{32,}`
- `pem_private_key`: `-----BEGIN\s+(?:RSA\s+)?PRIVATE\s+KEY-----`
- `ssh_private_key`: `-----BEGIN\s+(?:OPENSSH|EC|DSA)\s+PRIVATE\s+KEY-----`

**High / Block (3 patterns):**
- `google_api_key`: `AIza[0-9A-Za-z_-]{35}`
- `slack_token`: `xox[baprs]-[0-9a-zA-Z-]{10,}`
- `twilio_api_key`: `SK[a-fA-F0-9]{32}`

**High / Redact (2 patterns):**
- `bearer_token`: `Bearer\s+[a-zA-Z0-9_-]{20,}` -- Replaced with [REDACTED]
- `auth_header`: `(?i)authorization:\s*[a-zA-Z]+\s+[a-zA-Z0-9_-]{20,}`

**Medium / Warn (1 pattern):**
- `high_entropy_hex`: `\b[a-fA-F0-9]{64}\b` -- 64-char hex strings (potential secrets)

Scan flow per HTTP request (leak_detector.rs:285-316): URL first (most common exfiltration vector), then each header value, then body (uses `String::from_utf8_lossy` to prevent UTF-8 byte prefix bypass).

Redaction replaces matched ranges with `[REDACTED]` using non-overlapping range replacement (leak_detector.rs:359-381).

#### Injection Detection (sanitizer.rs:60-198)

**17 Aho-Corasick literal patterns:**

| Pattern | Severity | Category |
|---------|----------|----------|
| "ignore previous" | High | Direct instruction override |
| "ignore all previous" | Critical | Direct instruction override |
| "disregard" | Medium | Direct instruction override |
| "forget everything" | High | Direct instruction override |
| "you are now" | High | Role manipulation |
| "act as" | Medium | Role manipulation |
| "pretend to be" | Medium | Role manipulation |
| "system:" | Critical | System message injection |
| "assistant:" | High | Role injection |
| "user:" | High | Role injection |
| "<\|" | Critical | Special token delimiter |
| "\|>" | Critical | Special token delimiter |
| "[INST]" | Critical | Llama instruction token |
| "[/INST]" | Critical | Llama instruction token |
| "new instructions" | High | Instruction override |
| "updated instructions" | High | Instruction override |
| "```system" | High | Markdown system block |

**4 regex patterns:**
- Base64 encoded payloads (50+ contiguous chars) -- Medium
- Dynamic code invocation patterns -- High
- Code execution patterns -- High
- Null byte injection (\x00) -- Critical

**Escape behavior** (sanitizer.rs:253-284): Only triggered when Critical severity patterns are detected:
- Special token delimiters get backslash-escaped
- Null bytes removed
- Role markers (system:/user:/assistant: at line start) prefixed with `[ESCAPED]`

**Gap**: Medium and High severity patterns only generate warnings -- content passes through unmodified.

#### Policy Rules (policy.rs:131-201)

7 default rules applied to every tool output:

| Rule | Pattern | Severity | Action |
|------|---------|----------|--------|
| system_file_access | `/etc/passwd`, `/etc/shadow`, `.ssh/`, `.aws/credentials` | Critical | Block |
| crypto_private_key | `private.?key` or `seed.?phrase` near 64-char hex | Critical | Block |
| shell_injection | `; rm -rf` or `; curl ... \| sh` | Critical | Block |
| encoded_exploit | base64 decode calls, dynamic code invocation with base64, atob calls | High | Sanitize |
| sql_pattern | `DROP TABLE`, `DELETE FROM`, `INSERT INTO`, `UPDATE SET` | Medium | Warn |
| excessive_urls | 10+ URLs in sequence | Low | Warn |
| obfuscated_string | Any 500+ character run without whitespace | Medium | Warn |

### Layer 2: Shell Hardening (src/tools/builtin/shell.rs)

**Environment scrubbing** (line 151-196): Whitelist of safe environment variables:
- Core OS: PATH, HOME, USER, SHELL, TERM, LANG, LC_*
- Temp: TMPDIR, TMP, TEMP
- Toolchain: CARGO_HOME, RUSTUP_HOME, NODE_PATH, GOPATH, JAVA_HOME, PYTHONPATH
- Editor: EDITOR, VISUAL

Everything else stripped, including all KEY/TOKEN/SECRET vars.

**Three-tier command validation:**
1. **Blocked outright** (line 69-83): Destructive commands -- recursive root deletion, fork bombs, mkfs, dd zero-fill.
2. **Dangerous patterns** (line 86-102): Flagged for review -- sudo, curl-pipe-to-shell, access to /etc/passwd, ~/.ssh.
3. **Never auto-approve** (line 107-143): Always require human approval -- rm -rf, chmod 777, reboot, DROP TABLE, git push --force, git reset --hard.

### Layer 3: WASM Sandbox (src/tools/wasm/)

**Wasmtime engine configuration** (runtime.rs:113-145):
- Fuel consumption enabled (instruction counting)
- Epoch interruption enabled (wall-clock backup)
- Component model enabled (WASI Preview 2)
- Threads disabled (simplifies security model)
- Debug info disabled, persistent compilation cache enabled

**Dual timeout mechanism:**
- **Fuel**: 10M instruction budget. Traps when exhausted.
- **Epoch**: Background thread increments engine epoch every 500ms. Store sets deadline; traps when exceeded.
- **Tokio timeout**: 60s outer timeout wrapping invocation.

**DNS rebinding defense** (wrapper.rs:1006-1084): Every HTTP request validates resolved IPs against private ranges: loopback (127.0.0.0/8), RFC1918 (10/8, 172.16/12, 192.168/16), link-local (169.254/16), unspecified (0.0.0.0), CGNAT (100.64/10), IPv6 unique-local (fc00::/7), IPv6 link-local (fe80::/10). Checks ALL resolved IPs, not just the first.

**Credential injection flow** (wrapper.rs:265-290):
1. Inject credentials into URL placeholders
2. Validate URL against capability allowlist
3. Check per-invocation rate limit (max 50 HTTP requests)
4. Inject credentials into headers via pre-resolved host credentials
5. Leak detection on outbound request (URL, headers, body)
6. DNS rebinding check
7. Send request with 30s timeout
8. Leak detection on response body
9. Return sanitized response (credentials never visible to WASM)

### Layer 4: Docker Container Isolation (src/sandbox/)

**Container configuration** (container.rs:274-312):

| Setting | Value |
|---------|-------|
| Capabilities | cap_drop=ALL, cap_add=CHOWN |
| Security | no-new-privileges=true |
| Filesystem | readonly_rootfs (except FullAccess policy) |
| User | 1000:1000 (non-root) |
| Network | bridge mode, all HTTP proxied |
| Tmpfs | /tmp (512MB), ~/.cargo/registry (1GB) |
| Cleanup | auto_remove=true |

**Volume mounts per policy:**
- ReadOnly: /workspace:ro
- WorkspaceWrite: /workspace:rw
- FullAccess: /workspace:rw + /tmp:/tmp:rw (host /tmp -- a gap)

**Network proxy** (proxy/http.rs): All container HTTP routes through host-side proxy. Domain validation against allowlist (exact + wildcard). CONNECT for HTTPS. Credential injection via CredentialResolver trait. Default allowlist: package registries, docs sites, VCS, LLM APIs.

### Layer 5: Skills Trust and Attenuation (src/skills/)

| Trust Level | Source | Tool Access |
|-------------|--------|-------------|
| Trusted | User-placed in ~/.ironclaw/skills/ or workspace skills/ | All tools |
| Installed | Downloaded from ClawHub registry | Read-only tools only |

Attenuation (attenuation.rs:55-114): Minimum trust across active skills determines tool ceiling. LLM receives only filtered tool list -- cannot call tools it cannot see. Structural guarantee against privilege escalation via prompt injection.

**Anti-gaming constraints** (mod.rs:33-44): Max 20 keywords, 5 patterns, 10 tags per skill. Min 3-char length. Max 64 KiB file size.

### Layer 6: Web Auth (src/channels/web/auth.rs)

Bearer token with subtle::ConstantTimeEq -- timing-attack resistant for both header and query parameter auth.

### Threat Model: Attack Scenarios

**1. Unicode Homoglyph Bypass** (sanitizer.rs:160-162)
- Aho-Corasick does ASCII case-insensitive only. Cyrillic/Greek lookalike characters bypass all 17 patterns.
- Fix: Apply Unicode NFC normalization before matching.

**2. Multi-Step Injection** (sanitizer.rs:200-245)
- Sanitizer is stateless per-call. Split payloads across two tool outputs concatenate in LLM context.
- Fix: Cross-turn injection state or scan concatenated context.

**3. Container Privilege Escalation** (container.rs:282)
- CAP_CHOWN + SUID binary creation. Root process executing SUID binary gives container root.
- Fix: Remove CAP_CHOWN.

**4. Missing Seccomp** (container.rs:274-299)
- No seccomp allows ptrace (debug siblings), mount (host filesystem), bpf (kernel exploitation).
- Fix: Apply Docker default seccomp profile.

**5. Host /tmp Escape** (container.rs:265-271)
- FullAccess mounts host /tmp directly. Cron hooks or symlink races enable host code execution.
- Fix: Dedicated temp directory per container.

**6. WASM Supply Chain** (loader.rs:107-164)
- No cryptographic signature verification. Filesystem write access enables binary replacement.
- Fix: Ed25519 signatures with embedded trusted root key.

**7. TOCTOU in Approval** (dispatcher.rs:344-365)
- Session lock released between approval check (line 351) and decision (line 357-360).
- Fix: Hold lock across check-to-execution span.

**8. Upstream Error Credential Leak** (proxy/http.rs:433-439)
- API error messages containing keys (e.g., "Invalid key: sk-123...") forwarded to container.
- Fix: Leak detection on upstream error responses.

---

## 2. Agentic Loop Architecture

### Entry Point and Background Tasks

```
Agent::run() [agent_loop.rs:218]
|
+-- Start all channels (TUI, HTTP, Web, WASM)
+-- Spawn background tasks:
|   +-- Self-repair loop (every 30s)
|   |   Detects stuck jobs, recovery up to max retries
|   |   Detects broken tools (5+ failures), LLM-driven rebuild
|   |
|   +-- Session pruning (every 10min)
|   |   Removes sessions idle for > session_idle_timeout
|   |
|   +-- Heartbeat (every 30min)
|   |   Reads HEARTBEAT.md, runs LLM turn, notifies if findings
|   |
|   +-- Routine engine
|       Cron ticker: checks DB every 60s for due routines
|       Event matcher: checks every message against regex patterns
|
+-- Main select loop on merged message stream
    +-- Ctrl+C -> abort all, stop scheduler, shutdown channels
    +-- Message -> handle_message()
        +-- Parse submission type
        +-- Resolve/create session + thread
        +-- Check pending auth mode
        +-- UserInput -> Dispatcher.run_agentic_loop()
```

### Dispatcher: Core Reasoning Loop (src/agent/dispatcher.rs)

**Setup (lines 36-128):**
1. Detect group chat from metadata
2. Load workspace identity files as system prompt
3. Select active skills via keyword/regex scoring
4. Build skill context in XML delimiters with trust metadata
5. Create Reasoning engine
6. Set bounds: max_tool_iterations (default 50), nudge_at=N-1, force_text_at=N

**Each Iteration (lines 129-630):**

```
[1] Hard ceiling (line 133): iteration > max+1 -> error
[2] Interrupt check (line 142): thread.state == Interrupted -> error
[3] Cost guard (line 155): check daily budget + hourly rate
[4] Nudge at N-1 (line 164): "Provide your final answer"
[5] Force text at N (line 175): empty tools list
[6] Attenuation (line 181): filter tools by active skills trust
[7] LLM call (line 215): respond_with_tools()
[8] Record cost (line 217)
[9] Response handling:
    Text -> return to user (exit)
    ToolCalls -> three-phase execution:

    Phase 1: Preflight (Sequential) [lines 274-366]
      For each tool call:
        Run BeforeToolCall hook (modify params or reject)
        Check approval requirement
        STOP at first tool needing approval
        Remaining become deferred_tool_calls

    Phase 2: Execution (Parallel) [lines 368-489]
      Single: inline
      Multiple: tokio JoinSet
        Results slotted by original index
        Panics: slot filled with error (others unaffected)
        Per-tool timeout

    Phase 3: Post-flight (Sequential) [lines 491-627]
      Process results in original order
      Leak detection + sanitization on each output
      Check auth-awaiting state
      Add sanitized results to context
      If auth needed -> return instructions
      If approval needed -> create PendingApproval, return
```

### Cost Guard (src/agent/cost_guard.rs)

**Daily budget** (line 107-129):
- Atomic `budget_exceeded` flag for O(1) fast-path rejection
- Mutex-guarded daily total with midnight UTC reset
- 80% warning threshold

**Hourly rate** (line 132-146):
- VecDeque sliding window of Instant timestamps
- Drain entries older than 1 hour on each check

Cannot be bypassed: checked before every LLM call.

### Context Compaction (src/agent/compaction.rs)

Triggered at 80% of context limit (default 100K tokens):
- Over 95%: Truncate to 3 recent turns (emergency)
- Over 85%: LLM summarization (temperature=0.3), write to workspace daily log, keep 5
- 80-85%: Archive to workspace without summary, keep 10

### Job State Machine (src/context/state.rs)

```
Pending -> InProgress -> Completed -> Submitted -> Accepted
                      \> Failed
                      \> Stuck -> InProgress (recovery) \> Failed
                      \> Cancelled
```

Transition validation via pattern matching. Terminal: Accepted, Failed, Cancelled.

### Approval Flow

Tool requires approval -> PendingApproval created (snapshots context + deferred tools) -> thread state AwaitingApproval -> user /approve or /reject -> on approve: execute tool, run deferred tools, resume agentic loop -> --always adds to auto-approved set.

---

## 3. Containerization

### Two-Tier Model

**Tier 1: WASM** -- Lightweight sandbox. Fuel metering + epoch interruption + tokio timeout. Fresh instance per call. No filesystem. DNS rebinding defense. BLAKE3 integrity check.

**Tier 2: Docker** -- Full sandbox. Non-root UID 1000. Caps dropped. Read-only rootfs. Network proxy with credential injection. Bridge networking.

### Zero-Exposure Credential Model

Two independent implementations:

**WASM** (wrapper.rs:652-661, 265-290):
1. Pre-resolve secrets before WASM instantiation
2. Host function injects pre-resolved credentials into HTTP requests
3. WASM never sees raw secret values
4. Errors scrubbed with [REDACTED:name]

**Docker** (proxy/http.rs):
1. Container env has proxy URL only (no API keys)
2. Proxy intercepts HTTP, validates domain
3. CredentialResolver decrypts and injects into headers
4. Container never accesses credential values

**Known gaps**: DNS exfiltration (unmonitored), non-HTTP egress (unfiltered), no memory scrubbing (no zeroize), SSRF on OAuth token_url.

### Claude Code Bridge (src/worker/claude_bridge.rs)

Nested agent: IronClaw -> Docker -> Claude CLI. Each layer has independent access controls. Claude CLI configured via .claude/settings.json with explicit tool allowlist. Events streamed as NDJSON.

---

## 4. Architectural Patterns

### LLM Provider Chain (Decorator Pattern)

```
Raw Provider (NEAR AI / OpenAI / Anthropic / Ollama / etc.)
    v
RetryProvider: 1s*2^attempt backoff, +/-25% jitter, 100ms floor
    v
SmartRoutingProvider: simple->cheap, complex->primary, tools->primary
    v
FailoverProvider: sequential providers, atomic cooldown per-provider
    v
CircuitBreakerProvider: Closed->Open->HalfOpen->Closed state machine
    v
CachedProvider: SHA-256 key, LRU eviction, TTL expiry, text-only
```

Each implements LlmProvider trait. Transparently composable.

**Retry**: Classifies retryable (RequestFailed, RateLimited, InvalidResponse, Http, Io) vs non-retryable (AuthFailed, ContextLengthExceeded, ModelNotAvailable). Honors Retry-After headers.

**Smart routing**: Simple (<200 chars) -> cheap model. Complex (>1000 chars, code) -> primary. Tool calls always primary. Cascade mode: cheap first, escalate if uncertain.

**Failover**: Per-provider atomic failure counters. Cooldown activated at threshold. All cooled down -> try oldest-cooled. Lock-free via Ordering::Relaxed.

**Circuit breaker**: Mutex-guarded state. Open after failure_threshold (default 5). Recovery timeout (default 30s). Probe in HalfOpen. Closed after half_open_successes_needed (default 2).

**Cache**: Only caches complete() responses, NEVER tool completions (side effects).

### Trait-Based Polymorphism

| Trait | Implementations | Purpose |
|-------|-----------------|---------|
| LlmProvider | 6 raw + 5 decorators | LLM abstraction |
| Tool | 33 built-in + WASM + MCP | Capability abstraction |
| Channel | TUI, HTTP, Web, WASM | Input abstraction |
| Database | PostgreSQL, libSQL | Storage (7 sub-traits, ~60 methods) |
| EmbeddingProvider | OpenAI, NEAR AI, Ollama, Mock | Vector search |
| CredentialResolver | Env, No-op | Secret injection |

### Concurrency Model

- **RwLock** for read-heavy: tool registry, session map, WASM module cache
- **Mutex** for request/response: session, circuit breaker state
- **Atomics** for hot-path: failure counts, cooldowns, connection counts, budget flag
- **Arc** for zero-copy sharing across tasks
- **TOCTOU prevention**: Scheduler holds write lock across check-insert. Dispatcher does NOT (identified gap).
- **Fire-and-forget**: DB writes spawn tasks without awaiting. No backpressure.

---

## 5. Strengths

1. **Defense in depth**: Six independent layers. Compromising one does not compromise others.
2. **Zero-exposure credentials**: Two implementations. Even full RCE cannot extract secrets.
3. **Self-expanding tools**: LLM builds WASM tools at runtime without restart.
4. **Guaranteed termination**: Five independent mechanisms prevent infinite loops.
5. **Resilient LLM chain**: 5-layer provider chain handles real-world API instability.
6. **Skills attenuation**: Structural privilege isolation. LLM cannot see restricted tools.
7. **Hybrid search**: FTS + vector via RRF. Fallback-safe: FTS works if embeddings fail.
8. **Rust safety**: Memory safety, thread safety, type system. No unwrap in production.
9. **Dual DB backend**: PostgreSQL for production, libSQL for zero-dependency local mode.
10. **DNS rebinding defense**: All resolved IPs checked against private ranges.

---

## 6. Weaknesses

### Security
1. No Unicode normalization in sanitizer (homoglyph bypass)
2. No seccomp profile on containers (kernel syscall abuse)
3. CAP_CHOWN left enabled (SUID escalation)
4. No WASM signature verification (supply chain attack)
5. No memory scrubbing (secrets linger in memory)
6. Stateless sanitizer (multi-step injection blind spot)
7. Upstream errors can leak credentials
8. No ClawHub certificate pinning

### Architecture
9. Single-process (OOM or panic takes everything down)
10. In-memory session state (lost on restart)
11. JSON merge patch mismatch between backends
12. Binary skills trust (no granular capability grants)
13. libSQL backend incomplete (no vector search, secrets store)
14. Dead code (stub tool implementations)
15. No integration tests with real databases

### Operational
16. Fire-and-forget DB writes (unbounded task accumulation)
17. SSE 256-event buffer (slow clients lose events)
18. TOCTOU in approval flow

---

## 7. Scaling Analysis

### Current Design: Single-user, single-machine

MAX_PARALLEL_JOBS=5 (bounded by process resources). In-memory sessions (no sharing needed). RwLock registries (acceptable at low concurrency).

### Scales Well
- LLM provider chain (circuit breaker + failover + cache)
- WASM tool execution (stateless, fresh instance per call)
- Channel architecture (select_all merging, O(1) to add)
- Database abstraction (dual backend)

### Does Not Scale
- Single Tokio runtime (all jobs share one executor)
- In-memory sessions (no cross-instance sharing)
- RwLock registries (write contention during tool building)
- SSE broadcast (256-event buffer, 100-connection cap)
- Fire-and-forget writes (no backpressure)

### Path to Horizontal Scaling
1. Session state -> Redis (replace RwLock HashMap)
2. Job scheduling -> work queue (NATS, Redis Streams)
3. Workers as separate processes (orchestrator API already exists)
4. Tool registry -> database-backed (WasmToolStore already exists)
5. SSE -> event bus (Redis Streams, NATS JetStream)
6. DB writes -> bounded channel + batch flush

The orchestrator API (src/orchestrator/) is the natural split point.

---

## 8. Innovation Assessment

### Genuinely Novel

| Innovation | What Is New | Readiness |
|---|---|---|
| Self-expanding WASM tools | LLM generates, compiles, registers sandboxed tools at runtime | 60% |
| Skills trust attenuation | Minimum trust determines tool ceiling; structural privilege isolation | 70% |
| Zero-exposure credential injection | Two independent implementations; secrets never reach untrusted code | 75% |
| WASM channel system | Chat platforms as sandboxed plugins with fuel metering | 60% |
| Claude Code bridge | Nested agent architecture with three security boundaries | 50% |

### Smart Applications of Known Techniques

| Technique | Application |
|---|---|
| Decorator pattern | 5-layer LLM provider chain with independent resilience |
| Reciprocal Rank Fusion | Memory search combining FTS + vector; fallback-safe |
| Exponential moving average | Self-tuning cost/time predictions from actual outcomes |
| Cron + event-driven | Proactive execution; lightweight routines skip scheduler |
| IP range blocking | DNS rebinding defense checking ALL resolved IPs |

### Production Readiness

| Component | Ready | Blocking Gap |
|-----------|-------|-------------|
| Safety pipeline | 80% | Unicode normalization, cross-turn state |
| Shell hardening | 90% | None critical |
| WASM sandbox | 85% | Signature verification |
| Docker sandbox | 70% | Seccomp, CAP_CHOWN, user namespaces |
| Skills attenuation | 70% | Granular capabilities, revocation UX |
| Web auth | 95% | None critical |
| LLM provider chain | 90% | Observability |
| Cost guardrails | 85% | Per-job budget |
| Context compaction | 80% | Token estimation accuracy |
| Workspace/memory | 70% | libSQL vector search |
| Self-expanding tools | 60% | Error recovery, capability granting |
| Estimation/learning | 50% | Persistence, outlier detection |
