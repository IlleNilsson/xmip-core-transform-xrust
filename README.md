# xmip-core-transform-xrust

The xrust XSLT engine, a technology of
[xmip-core-transform](https://github.com/IlleNilsson/xmip-core-transform):
the first engine the Stylesheet unit chooses among, by a step's
`engine = "xrust"`. `xrust` (Apache-2.0) implements XSLT 1.0 over the XDM 3.1
data model, so a step needing a later version through it is refused when it
is opened. Its trees are built on `Rc`: a stylesheet is compiled when its step
is opened and once more on each thread that applies it, never per Message.

Offline (ADR-0045, amendment 2026-10-22): `xrust` reads nothing itself and
asks for every document an `xsl:include`, `xsl:import` or `document()`
names; this engine answers from the stylesheet's own folder and below
alone, and refuses `http:`, `https:` and every other address in words,
before anything is read.
## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
