# Linear Language Handoff

This document is a detailed handoff for the current `linear` language
prototype. It explains what the language is meant to become, what has been
built in the Rust scaffold, what semantics are currently intended, and what
work should come next.

The short version: Linear is intended to be a mostly functional,
linear-by-default language for writing programs that can later be lowered
efficiently to proving-oriented IR. The language should preserve programmer
intent longer than a conventional compiler pipeline, especially around state
threading, collections, iteration, and higher-level application logic.

## Product Goal

Linear is being designed for efficient proving, not primarily for native CPU
performance. That changes the language priorities:

- Avoid accidental mutable memory.
- Make value flow explicit enough that proving backends know what is truly
  updated, what is just observed, and what is consumed.
- Preserve high-level intent for optimizations instead of lowering early into
  low-level control flow and then trying to recover intent later.
- Let application code be expressive enough for real business logic, policies,
  graph-like state, maps/sets/lists, and iteration pipelines.
- Support convenient syntax, but keep the semantic core small and explicit.

The language should feel usable at the surface, with syntax inspired by Rust
and Swift, but the semantic model is purely functional and linear:

- Values are consumed when used.
- Returning a "changed" value means returning a new version.
- Returning an "unchanged" value means the value was threaded through.
- Dropping and copying are explicit semantic operations, controlled by
  capabilities.

## Repository Shape

Current crate modules:

- `src/types.rs`: type arena and type capabilities.
- `src/core.rs`: semantic core program, expressions, checker, builtins.
- `src/eval.rs`: interpreter for checked core programs.
- `src/frontend/ast.rs`: parsed frontend AST.
- `src/frontend/parse.rs`: Chumsky parser for the current surface syntax.
- `src/frontend/lower.rs`: frontend type/signature/body lowering into core.
- `src/id.rs`: typed ids for types, functions, globals, and values.
- `docs/core.md`: semantic core notes.
- `docs/surface.md`: current surface syntax notes.
- `bootstrap/*.lr`: sketches of future Linear code written in the developing
  syntax.
- `tests/*.rs`: tests for type semantics, core checking/eval, frontend parsing,
  and frontend lowering.

The parser is expected to remain Rust/Chumsky for now. The eventual goal is to
bootstrap most of the language/compiler logic in Linear itself, but parsing is
complex enough that it can remain a Rust component.

## Semantic Core

The semantic core is not meant to be a surface syntax. It is the small
intermediate language that all future syntax lowers into.

Core program pieces:

- type store;
- function definitions;
- global declarations and literal global definitions;
- statements assigning fresh value ids;
- explicit returned value ids.

Core functions:

- have typed input value ids;
- have output types;
- contain a list of single-assignment statements;
- return a list of live value ids.

The core checker enforces:

- every value id is defined before use;
- no value id is defined twice;
- every local value is consumed exactly once, either by an expression, `zap`, or
  function return;
- consumed values cannot be reused;
- result arities match expression result arities;
- return arities and types match function output types.

The core is intentionally strict. Surface syntax can be nicer, but it must lower
to explicit value flow.

## Type System

The foundational ordinary types are:

- `never`: no values;
- `unit`: one value.

User-defined ordinary types are built from:

- products;
- sums;
- functions.

The type system currently rejects recursive types by construction: type
definitions can only reference already-defined types. This means ordinary
recursive encodings like `List<T> = none + cons(T, List<T>)` are not available.
Collections are primitive type families instead.

Compact finite types exist for numbers:

- `Finite<N>` in the model;
- `u8`, `u16`, `u32`, `u64` in the current scaffold.

`u32` means a finite domain of size `2^32`, not an unbounded natural number.
Finite types are isomorphic to huge sums of `unit`, but they are represented
compactly and operated on with builtins.

Other built-in type families:

- `Bool`: a two-variant sum over `unit`;
- `Symbol`;
- `Text`;
- `List<T>` / `Vector<T>` / `Vec<T>`;
- `MutList<T>` / `MutVector<T>` / `MutVec<T>`;
- `HashMap<K, V>`;
- `MutHashMap<K, V>`.

## Capabilities And Linearity

Types have two structural capabilities:

- `Dup`: values of the type may be explicitly duplicated.
- `Zap`: values of the type may be explicitly dropped.

Absence of both means the type is linear.

Important distinction:

- `Dup`/`Zap` are type capabilities for local values.
- Global references are provenance/static-value rules. Referencing a global
  does not consume a local runtime resource, but the local value produced from
  that global must still obey linear rules.

Current capability behavior:

- `unit`, `never`, finite types, function types, `Symbol`, and `Text` support
  both `Dup` and `Zap`.
- products and sums derive capabilities structurally from components.
- immutable collections derive capabilities structurally from contents.
- mutable collections are linear regardless of contents.

There is no separate "unrestricted" flag in the type model. A type that can be
both duplicated and zapped is effectively unrestricted at the value level.

## Updates, Mutation, And Handles

The intended language is purely functional. What looks like mutation is value
threading:

```linear
fn step(mut state: State, config: Config, take event: Event) -> Decision
```

This means:

- `state` is consumed and returned changed;
- `config` is consumed and returned unchanged;
- `event` is consumed and not returned;
- `Decision` is the visible result.

At core level the function returns:

```text
(State, Config, Decision)
```

The visible surface `-> Decision` is only the suffix after implicit threaded
values. Hidden threaded returns always come first in parameter order.

There are no native references in the current design. Instead, updatable
collections/objects are represented by linear handles that must be threaded.
Keys or ids may be freely copied/stored when their type supports it, but the
base handle that grants access to updateable state remains linear.

This is intended to capture much of the expressive power of mutation while
staying functional:

- Other values may store keys into a collection or graph.
- Code that reads/updates by key must be passed the collection/graph handle.
- The handle returns unchanged for read-like operations and changed for updates.

This avoids Rust-like aliasing/borrow-checker complexity in the first design.
It also avoids true mutable references that can observe ambient mutation without
explicit state flow.

## Function Values And Higher-Order Functions

Function types are inhabited by static function identifiers. The language does
not create new runtime-defined functions in the semantic core:

- no nested function definitions that capture locals;
- no runtime lambdas;
- no currying that constructs new closures at runtime;
- no erased existential function payloads.

However, static function ids can be passed around as values. A value of function
type may be chosen by control flow or loaded from data, as long as its signature
is known.

Closures are intended as surface sugar over ordinary product types:

```text
Closure = { function: StaticFunctionId, captured: CapturedData }
```

Calling such a closure is just a call to a known apply function that receives
the closure data and ordinary arguments, and returns whatever captured data must
be threaded back. There does not need to be a privileged closure type in core.

This is deliberately less general than full higher-order functional languages,
but it should preserve efficient proving:

- static functions are easy to call/prove;
- closure payloads are explicit data;
- no dynamic code generation;
- no erased nested function towers unless represented explicitly by user data.

## Builtins

Current core builtins include:

- finite arithmetic: add, sub, mul;
- finite comparison: eq, lt;
- list operations: empty, push, len, get;
- hashmap operations: empty, insert, get_or, contains.

Finite arithmetic/comparison builtins are observer-style. They consume operands
and return operands unchanged before the visible result:

```text
add(lhs, rhs) -> (lhs, rhs, sum)
lt(lhs, rhs)  -> (lhs, rhs, bool)
```

This is intentional. Infix expressions are just sugar for ordinary primitive
function calls with value-flow metadata. The frontend does not invent `dup`
when a value is read by an observer op.

Collection builtins thread the collection handle:

- `len(list) -> (list, len)`;
- `get(list, index) -> (list, element)`;
- `insert(map, key, value) -> map`.

Some collection read operations currently require element/value capabilities.
For example, `ListGet` needs to produce an element while preserving the list,
so it requires the element type to support `Dup`.

## Surface Syntax

Surface syntax is intentionally still provisional, but the current direction is:

- Rust/Swift-like items;
- `fn`, `let`, `struct`, `enum`, `trait`, `impl`;
- braces for record products and blocks;
- parentheses for calls, params, grouping, and tuples;
- lowercase enum variants;
- dot syntax for enum constructors;
- `match` arms without Swift's `case`;
- no semicolons.

Examples:

```linear
struct User { id: u32, balance: u32 }
struct MyInt(u32)

enum Decision {
  allow { reason: u32 },
  deny,
  review { queue: u32, priority: u32 },
}

fn below_ten(x: u32) -> Bool {
  x < 10
}

fn reason(take decision: Decision) -> u32 {
  match decision {
    .allow { reason }: reason,
    .deny: 0,
    .review { queue, priority: p }: p,
  }
}
```

Flow markers:

- no marker: return unchanged;
- `mut`: return changed;
- `take`: consume and do not return.

Markers are accepted at parameter and call sites:

```linear
fn update(mut state: State, config: Config, take event: Event) -> Decision {
  apply(mut state, config, take event)
}
```

Call-site markers are mostly documentation because the callee signature
determines flow, but mismatches are rejected.

## Implemented Frontend Lowering

Current frontend lowering has three main entry points:

- `lower_type_items`: types only;
- `lower_module_signatures`: type plus function/global signature skeletons;
- `lower_module_bodies`: full lowering for the currently supported expression
  subset and core checking.

Implemented type lowering:

- type aliases;
- nominal structs;
- nominal enums;
- built-in finite integer aliases;
- `Bool`, `Symbol`, `Text`;
- collection type families listed above.

Parsed but rejected:

- user generic type definitions;
- generic functions;
- full generic impl/trait semantics.

Implemented body lowering:

- names and function params;
- `let` with name, wildcard, unit, tuple, and record patterns;
- typed scalar lets;
- integer literals;
- unit;
- direct function calls;
- hidden threaded return values;
- product constructors;
- enum constructors;
- `match` over enums;
- tuple/record payload patterns in `match`;
- finite infix ops over finite types;
- global references.

Currently intentionally rejected in body lowering:

- ordinary field access;
- ordinary method calls, except enum-constructor-like `Type.variant(...)`;
- `if`;
- string literals;
- collection operations from the surface;
- most complex expression forms around generics/traits.

## Current Tests

The test suite covers:

- type construction and capabilities;
- core linear checker behavior;
- evaluator behavior for products, sums, finite builtins, functions, globals,
  recursion, lists, and hashmaps;
- frontend parsing;
- frontend lowering for signatures, flow markers, products, enums, match, and
  arithmetic.

Run:

```sh
cargo test
```

Expected current result: all tests pass.

## Why This Is Not Just Rust Or Hylo

Linear borrows some surface ideas from Rust, Swift, and Hylo, but the intended
semantics are narrower and more proving-oriented:

- Like Rust, values can be linear and moves matter.
- Unlike Rust, there are no native references/borrows in the current semantic
  model.
- Like Hylo-style mutable value semantics, updates are value transformations.
- Unlike conventional functional languages, duplication/drop are explicit
  capabilities, not implicit structural freedoms.
- Unlike full HOF-heavy functional languages, runtime function values are static
  function ids, not arbitrary closures created at runtime.

The goal is not maximal generality. The goal is enough expressivity for real
applications while keeping proving efficient and lowering predictable.

## Important Design Decisions So Far

1. Variables are linear by default.

2. `Dup` and `Zap` are explicit capabilities. No implicit copying or dropping.

3. Updates are pure value transformations. The language should not need a Rust
   borrow checker at first.

4. Surface function outputs are visible outputs only. Core outputs include
   hidden threaded values first.

5. Infix operators are sugar for primitive calls. Primitive observer ops return
   their inputs unchanged, so reads do not force frontend-inserted `dup`.

6. Function values are static function identifiers. Rich closure syntax should
   lower to product data plus static apply functions.

7. Collections are primitive type families because recursive types are not in
   the core.

8. High-level iteration/collection-building intent should be preserved long
   enough for proving-oriented optimization.

## Major Open Questions

### Collection Builders

The language needs a way to build immutable/write-once collections without
exposing incremental mutable memory unnecessarily. Possible directions:

- builder syntax with `yield`;
- stream-to-collection forms;
- built-in collection comprehensions;
- explicit primitive builder values that are linear until finalized.

The desired property is that a collection can be produced as a whole before it
is made available for reads.

### Iteration

Iteration is a central reason this language exists. Lowering Rust/MIR-style
iterators too early loses intent and creates noisy `next`/`Option` shapes.

Future Linear should retain forms such as:

- map;
- filter;
- fold;
- scan;
- range iteration;
- indexed collection iteration;
- possibly graph/worklist iteration.

Some of these may be builtins or intent nodes rather than ordinary functions at
first. The goal is to let proving lower loops and masks efficiently.

### Traits And Generics

Traits currently parse and impl methods lower to names such as:

```text
Type.Trait.method
```

But trait semantics are not implemented. Open decisions:

- whether traits are just sets of associated function/type signatures;
- how `Self` is represented;
- monomorphization strategy;
- whether dynamic trait dispatch is forbidden entirely;
- how to report unsupported erased types clearly.

The likely rule is: no erased runtime type/function payloads. Everything must
monomorphize or be represented explicitly as sums/products/static function ids.

### Field Access And Projections

Core has product splitting/projection/reassembly primitives. Frontend lowering
does not yet support ordinary field access:

```linear
user.balance
```

This should lower through product splitting or projection/residuals while
respecting linearity. It is needed for realistic code.

### `if`

`if` already parses as an expression, and `else if` parses as nested `if`.
Lowering should be sugar over `match Bool`.

Both branches must return the same hidden threaded values plus the same visible
result shape.

### Closures

Closures should be designed as surface sugar over products and static function
ids. The compiler must decide:

- how closure structs are generated;
- how captured values are threaded back;
- how iteration APIs express closure requirements;
- how to reject unsupported nested/erased closure shapes clearly.

### Global Values

Current global definitions are literal expression trees only. If globals later
reference other globals, the checker needs dependency-cycle detection.

### Diagnostics

Current errors are structural Rust enum values, not polished user diagnostics.
Eventually the frontend should preserve spans and give targeted errors for:

- use after move;
- missing returned threaded value;
- accidental mutation of an unchanged argument;
- mismatched `take`/`mut` call markers;
- unsupported generics/traits;
- non-exhaustive match;
- bad pattern shape.

## Near-Term Implementation Plan

Recommended next steps:

1. Lower `if` as `match Bool`.

2. Lower ordinary product field access and update/reassembly sugar.

3. Add frontend access to collection builtins.

4. Add simple collection examples:
   - build a list;
   - read length;
   - get by index;
   - insert/get in hashmap.

5. Decide initial collection builder notation.

6. Decide initial iteration notation and preserve it in frontend/Core-like intent
   nodes rather than lowering immediately to recursive `next`.

7. Add a small end-to-end "application logic" example using structs, enums,
   hashmap/list, match, and threaded state.

8. Improve docs and tests around flow metadata inference.

## Handoff Notes For A New Implementer

Start by reading these files in order:

1. `docs/handoff.md`
2. `docs/core.md`
3. `docs/surface.md`
4. `src/types.rs`
5. `src/core.rs`
6. `src/eval.rs`
7. `src/frontend/ast.rs`
8. `src/frontend/parse.rs`
9. `src/frontend/lower.rs`
10. `tests/frontend_lower.rs`

The most important implementation invariant is:

> The frontend may hide value threading, but the core must make every consumed,
> returned, duplicated, and dropped value explicit.

When adding a surface feature, decide first what exact core value flow it
lowers to. If the feature reads a value without changing it, prefer a primitive
or function signature that returns the input unchanged. Do not insert `dup`
unless the source semantics really need two independent live values.

When adding a core builtin, define:

- input types;
- output types;
- whether inputs are returned unchanged, changed, or consumed;
- evaluator behavior;
- checker constraints;
- capability requirements.

Keep tests at both levels when possible:

- core tests for raw semantic behavior;
- frontend tests for source syntax lowering to executable core.

## Current Status Summary

As of this handoff:

- The Rust scaffold is a working semantic prototype.
- The parser is Chumsky-based and parses a broad syntax sketch.
- The core checker and evaluator are useful enough for executable tests.
- Frontend lowering supports a meaningful but still small subset.
- The language direction is settled enough to continue building features, but
  not settled enough to freeze syntax or trait/generic semantics.

The next meaningful milestone is an executable sample that uses:

- structs;
- enums and match;
- if;
- finite arithmetic/comparison;
- product field access;
- lists/hashmaps;
- threaded state through several functions.

That will expose the remaining ergonomic gaps before bootstrapping starts in
earnest.
