// Feed controlled byte sequences to the owned vt100 parser and print rows:
// splits "the wire was wrong" from "our parser mishandled the wire".
fn row0(parser: &vt100::Parser) -> String {
    parser.screen().rows(0, 88).next().unwrap_or_default()
}

fn main() {
    // Case A: the app's own sequence — 8 ASCII + wide pair + rest, then CUP
    // home and overwrite 9 ASCII (col 9 = lead of 读).
    let mut a = vt100::Parser::new(24, 88, 0);
    a.process("ow-cjk :读取rest".as_bytes());
    a.process(b"\x1b[1;1H#########");
    println!("A app-overwrite: {:?}", row0(&a));

    // Case B: the ConPTY repaint as captured on the wire — hashes then the
    // resurrected wide pair with no blank cell.
    let mut b = vt100::Parser::new(24, 88, 0);
    b.process("#########读取rest".as_bytes());
    println!("B conpty-wire  : {:?}", row0(&b));

    // Case C: expected-correct repaint — hashes, blank orphan, then 取.
    let mut c = vt100::Parser::new(24, 88, 0);
    c.process("######### 取rest".as_bytes());
    println!("C correct-wire : {:?}", row0(&c));
}
