# Core

This note describes the planned semantic core for the `linear` language. It is
not a surface syntax spec. The goal is to define the small semantic form that
future frontends and sugars lower into.

## Design Goals

- Variables are linear by default.
- The language is purely functional: updates are represented by consuming an
  old value and returning a new value.
- Surface syntax may look imperative or Hylo-like, but the semantic core should
  use explicit value flow.
- The core should preserve high-level intent for optimization when useful, for
  example iteration pipelines and collection builders.
- Sugars should ideally be reversible enough that different surface frontends
  can recover their preferred presentation from semantic metadata.

## Types

The initial ordinary types are:

- `Never`: no values.
- `Unit`: one value.

All ordinary user types are built from prior types using:

- sum: `A + B`
- product: `A * B`
- function: `A -> B`

The core also has compact finite-domain types:

- `Finite<N>`: a type with exactly `N` values, isomorphic to a sum of `N`
  `Unit` variants.

These are used for numbers. For example, `U32` is `Finite<2^32>`. The compiler
does not expand these types into enormous sums; arithmetic and comparison
operations over them are built-ins.

Fields, variants, and function inputs may be named. If a component is unnamed,
it receives its positional index as its name.

Examples:

```text
Bool = Unit + Unit
  variants: false, true

User = UserId * Balance * Locked
  fields: id, balance, locked
```

Recursive types are not allowed. A type definition may only reference types
already defined. This excludes ordinary inductive encodings such as:

```text
List<T> = Unit + (T * List<T>)
```

Collections will therefore be primitive type families; they are currently
removed from the scaffold pending redesign (see Collections below).

The current Rust scaffold also has primitive `Symbol`, `Text`, and named
opaque primitive types (including a builtin linear `Token`). `Symbol`,
`Text`, finite types, function types, `Unit`, and `Never` support explicit
`Dup` and `Zap`.

Function types are inhabited only by global function identifiers. Function types
are unary `A -> B`; multi-input or multi-output core functions are represented
as first-class values by packing their inputs and outputs into products. Zero
inputs or outputs pack as `Unit`, one input or output is used directly, and
multiple inputs or outputs pack into a product in declaration order.

The language does not construct new runtime functions with lambdas, currying,
partial application, or nested function definitions in the semantic core.
Closure syntax is surface sugar over product values and global apply functions.

## Values

The only primitive ordinary value is the value of `Unit`.

Other values are built by:

- sum introduction;
- product introduction;
- function inputs;
- global constants/functions;
- built-in collection operations.

The core program contains function definitions and non-function globals.
Non-function globals can be plain declarations or definitions.

A global declaration is a named typed symbol:

```text
global root : Root
```

A global definition additionally gives an initialized value body. The current
Rust scaffold supports literal global expression trees:

- `Unit`;
- finite literals;
- symbol and text literals;
- static function identifiers;
- product construction;
- sum injection.

These literal trees do not reference other globals, so they are acyclic by
construction. Static function identifiers are checked against the function type
stored in the global expression. If later global definitions can reference other
globals, the checker must reject recursive dependency cycles.

Referencing a global produces a local value of its type without consuming a
local runtime resource. That local value is then ordinary linear data and must
be consumed exactly once unless its type supports `Dup`/`Zap`.

Function definitions may be recursive or mutually recursive. Nontermination is
allowed; for proving, a nonterminating execution is a completeness failure for
that chosen input.

## Linearity

Every local value is linear unless the expression checker can prove it is a
static/global value rather than a runtime-owned local resource.

Using a value consumes it. A consumed value cannot be used again.

Two special traits control structural rules:

```text
Dup<T> : dup : T -> T * T
Zap<T> : zap : T -> Unit
```

`Dup` permits explicit duplication. `Zap` permits explicit dropping.
In the Rust scaffold, `Unit` is represented as zero core values for this
purpose, so a `Zap` expression consumes its input and binds no result ids.

Types with both traits behave like ordinary copyable/dropable values, but the
core still models duplication and dropping explicitly unless later optimization
erases them.

The type system does not model `unrestricted` as a third capability. Absence of
`Dup` and `Zap` means the type is linear. Having both `Dup` and `Zap` means the
type supports explicit copy/drop. Separately, global symbols and static function
identifiers are a value/provenance rule: referencing them does not consume a
local runtime resource.

Declared capabilities on products, sums, and collections cannot exceed their
structural capabilities. This prevents a wrapper around linear state from
declaring `Dup` or `Zap` and forging extra handles or silently dropping state.
Opaque primitive types may declare capabilities axiomatically.

## Functions

A function consumes all inputs it uses and returns all values that remain
available to the caller.

Core functions are lists of expressions. Each expression can use function inputs
or prior expression identifiers. Every input to an expression is consumed.

The current Rust scaffold represents a function as:

- input parameters with explicit value ids and types;
- output types;
- a list of statements, each assigning fresh value ids;
- a list of returned value ids.

Function and non-function global names currently share one namespace.

The checker enforces single assignment, no use after consume, no implicit
copying, no implicit dropping, and output type agreement.

Core identifiers are single-assignment. Surface syntax may reuse names:

```text
x = update(x)
```

but this lowers to fresh core identifiers:

```text
x0 = input
x1 = update(x0)
```

Returning an input "unchanged" is not primitive. It is proven by following value
flow. For example:

```text
preserve_and_inc : T -> T * Int
  (x0, x1) = dup(x)
  y = add(x1, 1)
  return (x0, y)
```

Metadata may record that `x0` is an unchanged return of input `x`, while `y`
is computed from another copy. This metadata supports surface sugars and docs.

## Version Metadata

Functions carry metadata describing how outputs relate to inputs:

- output `o` is the same version of input `i`;
- output `o` is an updated version of input `i`;
- output `o` is derived from input `i`;
- input `i` is consumed/sunk and not returned.

This is implemented in `src/flow.rs` as *path provenance*. Every value is
classified as `Same(place)` — provably the same version of the value at a
place, where a place is a parameter plus a path of field/variant steps into
it — or `Context(whole, hole)` — a one-hole context of a place — or
unproven. Builtins declare their flow axiomatically: finite
arithmetic/comparison ops return both operands unchanged, `dup` gives both
copies the source version, `next` returns a changed version.

The structure rules mirror focus/plug exactly:

```text
focus:  Same(w)            ->  Same(w.f) + Context(w, f)
plug:   Context(w, f) + Same(w.f)              -> Same(w)
split:  Same(w)            ->  Same(w.0) ... Same(w.n-1)
build:  Same(w.0) ... Same(w.n-1) at w's type  -> Same(w)
match:  arm k of Same(w) binds payload Same(w.k)
inject: Same(w.k) at variant k of w's type     -> Same(w)
```

So decompose-and-recompose round trips are recognized as identity at any
nesting depth, and rebuilding a *different* (even structurally identical)
nominal type is not, because the build/plug/inject rules compare against the
type at the source place.

Function summaries state each output slot's provenance in terms of the
callee's own parameters; call sites substitute (paths compose), so a context
can travel out of a helper, through the caller, and back into a plug.
Summaries are computed by a fixpoint over the call graph, so recursion and
mutual recursion converge; a path that never terminates constrains nothing
and satisfies any contract vacuously.

Frontend flow markers are checked against these summaries after body lowering:

- an unmarked (borrow) parameter whose hidden output slot is not provably the
  same version is a hard error;
- a `mut` parameter whose slot is provably the same version on every path is
  reported as actually being a borrow (a warning, not an error, until surface
  code can express real updates);
- a `take` parameter whose exact version provably escapes into any output is
  reported as moved-through rather than taken; consumption itself is already
  enforced by linearity.

Known remaining coarseness: focusing *into* a context (a hole inside a hole
of the same product) degrades to unproven because field indices shift around
the first hole, and reaching through a context passed to a callee does too.
Both are conservative, not unsound.

## Focus And Plug (One-Hole Contexts)

Whole-product splitting consumes a product and returns all fields:

```text
(a, b, c) = split(product)
```

Taking out a *single* field is `focus`, and its inverse is `plug`:

```text
(part, context) = focus_f(whole)
whole2          = plug_f(context, part)
```

The context is the one-hole context of the product at that field — in
derivative-of-types terms, the derivative of the product type: an ordinary
product of the remaining fields, names and order preserved. No special hole
type exists; the compiler creates only the context product types a program
actually uses. The checker requires the context type to exactly match the
original product with the focused field removed. The field index is static,
so for products the address lives in the instruction, not the value.

Example:

```text
User = { id: UserId, balance: Balance, locked: Bool }

focus_balance : User -> Balance * { id: UserId, locked: Bool }
plug_balance  : { id: UserId, locked: Bool } * Balance -> User
```

Nested access is repeated focusing, and plugging in reverse order — the
ordering is forced by the types, since the outer plug needs the inner plug's
result:

```text
(profile, user_ctx)    = focus_profile(user)
(address, profile_ctx) = focus_address(profile)
address2 = normalize(address)
profile2 = plug_address(profile_ctx, address2)
user2    = plug_profile(user_ctx, profile2)
```

Surface sugar will present this as borrowing or updating a nested field. The
value-flow analysis (see Version Metadata) recognizes a focus/plug round trip
that puts the same version back as returning the same version of the whole —
that is what lets a function that reads `user.balance` verify as a *borrow*
of `user`. When collections return, they get the same operation pair with a
dynamic address carried inside the context value.

## Sums And Pattern Matching

Matching a sum consumes the scrutinee and introduces the selected variant
payload in the chosen branch.

The current Rust scaffold implements exhaustive matching with explicit state
threading:

- every variant must have exactly one arm;
- matching captures all currently live surrounding local values;
- each arm starts with its variant payload plus those captured values;
- every arm must consume, return, or explicitly `zap` the payload and every
  captured value;
- every arm must return the same output types, which become the match result.

Operationally, a match is therefore a structured control-flow join. It consumes
the pre-match environment and replaces it with whichever successor values the
chosen arm returns.

## Traits

A trait is a named bundle of associated types and functions parameterized by
`Self`.

Example:

```text
trait Eq {
  eq : Self * Self -> Bool
}
```

An implementation supplies those associated definitions for a concrete type.

Trait signatures are linear signatures. If an operation conceptually preserves
an input, the signature and/or metadata must show the returned version.

`Dup`, `Zap`, `Eq`, ordering, hashing, and collection-specific interfaces can
all be modeled this way.

## Closures

There are no runtime-defined functions in the semantic core.

Closure syntax lowers to:

- a product containing captured values;
- a global apply function for that closure site.

Example surface idea:

```text
|x| x + scale
```

Core shape:

```text
ClosureK = { scale: Scale }
applyK : ClosureK * X -> ClosureK * Y
```

The closure product is returned from each use so captured values are threaded
across repeated calls. If the caller does not need the captured values, it may
eventually `zap` the final closure when allowed.

No erased closure type exists. Storing different closure shapes in one value
requires an explicit sum type over the finite cases.

## Collections

Collections have been removed from the scaffold while they are redesigned.
The previous primitive `List`/`HashMap` families, their builtins, and the
extractive read semantics are gone; nothing in the core references them.

The redesign direction is recorded in the working discussion: immutable
collections observe (path reads that duplicate only `Dup` leaves) or are
consumed; extraction and substitution live on mutable collections; take-apart
operations return the part plus a one-hole context (the type derivative), and
reassembly is `plug`, with product projection/insertion as the static-address
special case. Same-version flow metadata is the mechanism that will make
read-only plugging checkable.

Until then, the only builtins are scalar:

- finite arithmetic and comparisons: `add`, `sub`, `mul`, `eq`, `lt`, all
  observer-style (operands are returned unchanged before the visible result);
- `next`, a toy update builtin that consumes a finite value and returns a
  changed version (+1 modulo the cardinality). It exists so value-flow
  checking has an axiomatic "changed" primitive to recurse to until real
  update builtins land.

There is also a builtin `Token` primitive type with neither `Dup` nor `Zap`,
so surface programs can exercise strict linearity without collections.

## Evaluation

The Rust scaffold includes an interpreter for checked core programs. It is not
the final proving backend; it is a way to make the semantic core executable
while the language is still being designed.

The evaluator currently supports:

- direct and recursive function calls, with a step limit;
- static function values and dynamic calls through function values;
- global definitions;
- finite arithmetic, comparisons, and `next`;
- products, sums, exhaustive matching, `dup`, and `zap`.

Finite arithmetic is modular over the finite type cardinality. Boolean visible
results are represented as a two-variant sum over `Unit`, with variant `0` as
false and variant `1` as true.

## Iteration And Intent

Iteration can be modeled with streams and recursive functions, but the compiler
should not lower all high-level iteration to `next` too early. That would lose
intent and recreate the problem of rediscovering `map`, `filter`, and `fold`
from lower-level control flow.

The semantic IR should retain intent nodes for common iteration/building forms,
such as:

- `map`;
- `filter`;
- `fold`;
- `range`;
- `zip`;
- collection `build` / `yield`.

These forms are typechecked like functions, but they may remain as explicit
semantic nodes until an optimization/lowering phase decides how to implement
them.

Less common recursive stream programs are still allowed, but may not receive
the same optimization quality.

## Collection Building

(Pending the collections redesign.) The earlier sketch remains the baseline:
building goes through linear builder values (`new_builder` / `yield` /
`finish`), with `collect` defined as a fold over builder operations so a type
has `collect` exactly when it has a builder. Whether builders are a restricted
mode of mutable collections plus a `freeze`, and whether reads during build
are permitted, are open questions tracked in the redesign.

## Open Questions

- Which primitive collection families are required initially?
- Which iteration intent nodes are kept in the semantic IR?
- Exact syntax and metadata format for unchanged/updated output tracking.
- Exact trait derivation rules for `Dup`, `Zap`, `Eq`, `Hash`, and ordering.
- How much reversible source-sugar metadata should the canonical form preserve?
- Whether streams are primitive collections, primitive views, or just a
  lowering target for retained iteration forms.
