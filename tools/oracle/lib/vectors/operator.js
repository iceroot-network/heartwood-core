"use strict";
// V35's environment file records: `env.stringify` (a variable map written as the reference's
// envfile writes it, `stringifySync(...).trim()`) and `env.parseCli` (a file's text as the
// reference's CLI reads it with envfile's `parseFileSync`), with the lines the node's own parser
// reads otherwise noted `cliEnvParser`.

module.exports = async function operatorVectors({ rec, must, shared }) {
    const out = [];
    const H = shared.height;
    const push = async (op, input, name) => out.push({ ...must(await rec(op, H, input)), name });

    // Variables assigned to an empty object, or (with a file) to the file's variables.
    const maps = [
        ["no variables", []],
        ["one variable", [["A", "1"]]],
        ["variables in the order they came", [["Z", "1"], ["B", "2"], ["M", "3"]]],
        ["a key again takes its place", [["A", "1"], ["B", "2"], ["A", "3"]]],
        ["numbers", [["A", 1], ["B", 1.5], ["C", -7], ["D", 1e21], ["E", 0], ["F", 1e-7]]],
        ["array-index keys come first, in numeric order", [["b", "1"], ["10", "2"], ["2", "3"], ["01", "4"], ["4294967294", "5"], ["4294967295", "6"], ["0", "7"], ["a", "8"]]],
        ["an empty key is left out", [["", "x"], ["A", "1"]]],
        ["an empty value", [["A", ""], ["B", "2"]]],
        ["__proto__ is no key", [["__proto__", "x"], ["A", "1"]]],
        ["prototype names", [["constructor", "2"], ["toString", "three"], ["valueOf", "true"]]],
        ["spaces and quotes stay", [["A B", " x  y "], ["C", '"q"'], ["D", "'s'"]]],
        ["a value with a line break", [["A", "one\ntwo"], ["B", "3"]]],
        ["a value that is only white space", [["A", "  "]]],
        ["whitespace at the ends is trimmed", [["A", "\n\n"], ["B", "\t"]]],
        ["JavaScript's white space at the ends", [["﻿A", "1"], ["B", "　"]]],
        ["U+0085 is not white space", [["A", "1"], ["B", "\u0085"]]],
        ["a hash and an equals sign", [["A#B", "c=d#e"], ["E", "="]]],
        ["unicode", [["ÉTAT", "été"], ["😀", "😁"]]],
        ["a key with a colon", [["A:B", "1"]]],
        ["big numbers", [["A", 9007199254740993], ["B", 1.7976931348623157e308], ["C", 5e-324]]],
    ];
    for (const [name, vars] of maps) await push("env.stringify", { vars }, name);

    // The file's variables first: comments and blank lines are dropped, the order is JavaScript's.
    const files = [
        ["a comment and a blank line are dropped", "# a comment\n\nA=1\nB=2\n", [["C", "3"]]],
        ["an indented comment is dropped", "  # note\nA=1\n#B=2\n", [["A", "9"]]],
        ["a set variable keeps its place", "A=1\nB=2\nC=3\n", [["B", "x"]]],
        ["a set number", "A=1\n", [["A", 42]]],
        ["a colon separator is written with an equals sign", "A:1\nB: two\n", [["C", "3"]]],
        ["array-index keys move first", "b=1\n10=2\n2=3\n", [["a", "4"]]],
        ["CRLF lines", "A=1\r\nB=two\r\n", [["C", "3"]]],
        ["a file with only comments", "# one\n# two\n", [["A", "1"]]],
        ["an empty file", "", [["A", "1"]]],
        ["a value with an equals sign", "A=b=c\n", [["D", "e=f"]]],
        ["spaces around the separator are trimmed", " A = 1 \n", [["B", "2"]]],
        ["a quoted, an empty and a zero-led value, which the node reads otherwise", 'A="x y"\nB=\nC=007\n', [["D", "1"]]],
        ["a JSON file, which only the reference's CLI reads as JSON", '{"A":1,"B":"two"}', [["C", "3"]]],
    ];
    for (const [name, file, vars] of files) await push("env.stringify", { file, vars }, name);

    // A file's text as the reference's CLI reads it.
    const texts = [
        ["pairs", "A=1\nB=two\nC=true\nD=false"],
        ["colon separator", "A:1\nB: two"],
        ["the first separator splits", "A=b=c\nB:c=d\nC=d:e"],
        ["comments and blank lines", "# a comment\n\nA=1\n  # indented comment\n#B=2"],
        ["a hash inside the key", "A#B=1\nC=d#e"],
        ["quotes", 'A="x"\nB=" x "\nC=""\nD="\nE=\'x\'\nF= "x"\nG="a"b"\nH="""'],
        ["digits", "A=0\nB=00\nC=007\nD=42\nE= 42 \nF=1e5\nG=-1\nH=1.5\nI=99999999999999999999999\nJ=9007199254740993"],
        ["booleans", "A=true\nB=false\nC=True\nD=FALSE\nE= true"],
        ["empty keys and values", "A=\n=x\n  =y\nB=  \nC"],
        ["CRLF lines", "A=1\r\nB=two\r\nC\r=3"],
        ["line separators", "A=a b\nB=c d\nC =e"],
        ["whitespace JavaScript trims", "﻿A=1\n B =　two　\n\u000bC\u000c=3"],
        ["whitespace JavaScript keeps", "A\u0085=1\nB=\u0085two\u0085"],
        ["a repeated key keeps its first place", "A=1\nB=2\nA=3"],
        ["array-index keys first, in numeric order", "b=1\n10=2\n2=3\n01=4\n4294967294=5\n4294967295=6\n0=7\na=8"],
        ["prototype names", "__proto__=1\nconstructor=2\ntoString=three\nhasOwnProperty=true"],
        ["export and spaces in keys", "export A=1\nB C=2\n  D  =  3  "],
        ["tabs", '\tA\t=\t1\t\nB=\t"x"\t'],
        ["unicode", "ÉTAT=été\n😀=😁"],
        ["a final line break", "A=1\n"],
        ["an empty text", ""],
        ["a file that is JSON", '{"A":1,"B":"two"}'],
        ["a file that is a JSON string", '"text"'],
        ["a file that is JSON with a number", "42"],
        ["a JSON array", "[1,2]"],
        ["text that starts like JSON", "{A=1\nB=2"],
        ["a line with no separator", "A\nB=1\nC"],
    ];
    for (const [name, text] of texts) await push("env.parseCli", { text }, name);
    return out;
};
