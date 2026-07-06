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

- `never`: no values.
- `unit`: one value.

All ordinary user types are built from prior types using:

- sum: `A + B`
- product: `A * B`
- function: `A -> B`

The core also has compact finite-domain types:

- `Finite<N>`: a type with exactly `N` values, isomorphic to a sum of `N`
  `unit` variants.

These are used for numbers. For example, `u32` is `Finite<2^32>`. The compiler
does not expand these types into enormous sums; arithmetic and comparison
operations over them are built-ins.

Fields, variants, and function inputs may be named. If a component is unnamed,
it receives its positional index as its name.

Examples:

```text
Bool = unit + unit
  variants: false, true

User = UserId * Balance * Locked
  fields: id, balance, locked
```

Recursive types are not allowed. A type definition may only reference types
already defined. This excludes ordinary inductive encodings such as:

```text
List<T> = unit + (T * List<T>)
```

Collections are therefore primitive types, described separately below.

The current Rust scaffold also has primitive `Symbol`, `Text`, ordered
`List<T>`/`Vector<T>`, `HashMap<K, V>`, and named opaque primitive types.
`Symbol`, `Text`, finite types, function types, `unit`, and `never` support
explicit `Dup` and `Zap`. Collections come in two kinds:

- immutable collections, whose `Dup`/`Zap` capabilities are derived
  structurally from their contents;
- mutable collections, which are linear even when their contents are copyable.

Function types are inhabited only by global function identifiers. Function types
are unary `A -> B`; multi-input or multi-output core functions are represented
as first-class values by packing their inputs and outputs into products. Zero
inputs or outputs pack as `unit`, one input or output is used directly, and
multiple inputs or outputs pack into a product in declaration order.

The language does not construct new runtime functions with lambdas, currying,
partial application, or nested function definitions in the semantic core.
Closure syntax is surface sugar over product values and global apply functions.

## Values

The only primitive ordinary value is the value of `unit`.

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

- `unit`;
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
Zap<T> : zap : T -> unit
```

`Dup` permits explicit duplication. `Zap` permits explicit dropping.
In the Rust scaffold, `unit` is represented as zero core values for this
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

Functions can carry metadata describing how outputs relate to inputs:

- output `o` is the same version of input `i`;
- output `o` is an updated version of input `i`;
- output `o` is derived from input `i`;
- input `i` is consumed/sunk and not returned.

The compiler should infer this metadata by following definitions down to base
built-in operations. Built-ins declare the metadata axiomatically.

This metadata is not required for type soundness. It is for diagnostics,
documentation, surface sugar, and optimization.

## Products And Projection

The first implementation supports whole-product splitting:

```text
(a, b, c) = split(product)
```

This consumes the product and returns all fields.

It also supports field projection and reassembly. Projecting a field consumes
the product and returns:

- the selected field value;
- a residual product containing the remaining fields.

Example:

```text
User = { id: UserId, balance: Balance, locked: Bool }

take_balance : User -> Balance * { id: UserId, locked: Bool }
put_balance  : Balance * { id: UserId, locked: Bool } -> User
```

No hole type is needed. Residual products are ordinary product types. The
compiler creates only the residual product types actually needed by a program.
The checker currently requires the residual product type to exactly match the
original product with the selected field removed, preserving remaining field
names and order.

Nested projection is repeated decomposition and reassembly:

```text
(profile, user_rest) = take_profile(user)
(address, profile_rest) = take_address(profile)
address2 = normalize(address)
profile2 = put_address(address2, profile_rest)
user2 = put_profile(profile2, user_rest)
```

Surface sugar may present this as borrowing or updating a nested field.

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

Recursive user types are banned, so collections are primitive families. The
initial implemented collection families are:

- `List<T>` / `Vector<T>`;
- `HashMap<K, V>`.

Collection operations are ordinary linear functions at the semantic boundary:
an operation consumes a collection/root/builder and returns the next version
when appropriate.

This is true for both collection kinds. For immutable collections, an update
operation returns a new immutable value. For mutable collections, the same
shape represents the next version of the unique mutable handle. The evaluator
uses the same runtime representation for both; the distinction is type-level
and later metadata/lowering can use it.

The current built-ins are:

- finite arithmetic and comparisons: `add`, `sub`, `mul`, `eq`, `lt`;
- list: `empty`, `push`, `len`, `get`;
- hashmap: `empty`, `insert`, `get_or`, `contains`.

Finite arithmetic and comparison builtins are observer-style operations: they
consume both operands and return both operands unchanged, followed by the
visible result. For example, finite `add` has core result shape
`(lhs, rhs, sum)`, and finite `lt` has result shape `(lhs, rhs, bool)`.

Read-like operations are extractive. For example, `ListGet` consumes
`list, index` and returns `list, element`, where the returned list is the
residual collection with the element moved out. `HashMapGetOr` likewise returns
the residual map and the moved-out or default value. The residual has the same
surface/core type as the original collection; any hole or deletion bookkeeping
is a collection-builtin/backend responsibility, not a user-visible type.

Because elements are moved out, these operations do not require element/value
`Dup`. If a program wants both the extracted element and a restored equivalent
collection, it must explicitly duplicate the element, then reinsert one copy.

Built-in collection operations will declare version metadata axiomatically.

## Evaluation

The Rust scaffold includes an interpreter for checked core programs. It is not
the final proving backend; it is a way to make the semantic core executable
while the language is still being designed.

The evaluator currently supports:

- direct and recursive function calls, with a step limit;
- static function values and dynamic calls through function values;
- global definitions;
- finite arithmetic and comparisons;
- products, sums, exhaustive matching, `dup`, and `zap`;
- lists/vectors and hashmaps.

Finite arithmetic is modular over the finite type cardinality. Boolean visible
results are represented as a two-variant sum over `unit`, with variant `0` as
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

Building primitive collections requires primitive construction APIs. The
preferred model is a linear builder:

```text
new_builder : unit -> Builder<C>
yield       : Builder<C> * Item -> Builder<C>
finish      : Builder<C> -> C
```

Surface syntax may provide:

```text
build List<Int> {
  yield 1
  yield 2
}
```

which lowers to builder operations.

Unfold/co-recursive style builders may be added as intent nodes, but builder
values are the minimal semantic mechanism.

## Open Questions

- Which primitive collection families are required initially?
- Which iteration intent nodes are kept in the semantic IR?
- Exact syntax and metadata format for unchanged/updated output tracking.
- Exact trait derivation rules for `Dup`, `Zap`, `Eq`, `Hash`, and ordering.
- How much reversible source-sugar metadata should the canonical form preserve?
- Whether streams are primitive collections, primitive views, or just a
  lowering target for retained iteration forms.
