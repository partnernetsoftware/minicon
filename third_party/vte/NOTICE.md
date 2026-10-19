# Owned lexer provenance

Based on `alacritty/vte` 0.15.0, upstream revision
`3b3da71c34cc1256c7e20981cf03f8eb95e08ffc`, crates.io package checksum
`a5924018406ce0063cd67f8e008104968b74b563ee1b85dde3ed1f7cb87d3dbd`.
Upstream source and MIT/Apache notices are retained. Local changes expose exact
input boundaries and side-effect-free byte preview, using the existing grammar.
No Windows semantics are added here. Ownership and qualification live in
[terminal rendering](../../prd/PRD_02_24_con_terminal.md).
