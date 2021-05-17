const EX2_1 : &'static str =
    "- Mark McGwire\n- Sammy Sosa\n- Ken Griffey";

const EX2_2 : &'static str =
    "hr:  65    # Home runs\navg: 0.278 # Batting average\nrbi: 147   # Runs Batted In";

const EX2_3 : &'static str =
    "american:\n- Boston Red Sox\n- Detroit Tigers\n- New York Yankees\nnational:\n- New York Mets\n- Chicago Cubs\n- Atlanta Braves";

const EX2_4 : &'static str =
    "-\n  name: Mark McGwire\n  hr:   65\n  avg:  0.278\n-\n  name: Sammy Sosa\n  hr:   63\n  avg:  0.288";

const EX2_5 : &'static str =
    "- [name        , hr, avg  ]\n- [Mark McGwire, 65, 0.278]\n- [Sammy Sosa  , 63, 0.288]";

const EX2_6 : &'static str =
    "Mark McGwire: {hr: 65, avg: 0.278}\nSammy Sosa: {\n    hr: 63,\n    avg: 0.288\n  }";

const EX2_7 : &'static str =
    "# Ranking of 1998 home runs\n---\n- Mark McGwire\n- Sammy Sosa\n- Ken Griffey\n\n# Team ranking\n---\n- Chicago Cubs\n- St Louis Cardinals";

const EX2_8 : &'static str =
    "---\ntime: 20:03:20\nplayer: Sammy Sosa\naction: strike (miss)\n...\n---\ntime: 20:03:47\nplayer: Sammy Sosa\naction: grand slam\n...";

const EX2_9 : &'static str =
    "---\nhr: # 1998 hr ranking\n  - Mark McGwire\n  - Sammy Sosa\nrbi:\n  # 1998 rbi ranking\n  - Sammy Sosa\n  - Ken Griffey";

const EX2_10 : &'static str =
    "---\nhr:\n  - Mark McGwire\n  # Following node labeled SS\n  - &SS Sammy Sosa\nrbi:\n  - *SS # Subsequent occurrence\n  - Ken Griffey";

const EX2_11 : &'static str =
    "? - Detroit Tigers\n  - Chicago cubs\n:\n  - 2001-07-23\n\n? [ New York Yankees,\n    Atlanta Braves ]\n: [ 2001-07-02, 2001-08-12,\n    2001-08-14 ]";

const EX2_12 : &'static str =
    "---\n# Products purchased\n- item    : Super Hoop\n  quantity: 1\n- item    : Basketball\n  quantity: 4\n- item    : Big Shoes\n  quantity: 1";

const EX2_13 : &'static str =
    "# ASCII Art\n--- |\n  \\//||\\/||\n  // ||  ||__";

const EX2_14 : &'static str =
    "--- >\n  Mark McGwire's\n  year was crippled\n  by a knee injury.";

const EX2_15 : &'static str =
    ">\n Sammy Sosa completed another\n fine season with great stats.\n \n   63 Home Runs\n   0.288 Batting Average\n \n What a year!";

const EX2_16 : &'static str =
    "name: Mark McGwire\naccomplishment: >\n  Mark set a major league\n  home run record in 1998.\nstats: |\n  65 Home Runs\n  0.278 Batting Average\n";

const EX2_17 : &'static str =
    "unicode: \"Sosa did fine.\\u263A\"\ncontrol: \"\\b1998\\t1999\\t2000\\n\"\nhex esc: \"\\x0d\\x0a is \\r\\n\"\n\nsingle: '\"Howdy!\" he cried.'\nquoted: ' # Not a ''comment''.'\ntie-fighter: '|\\-*-/|'";

const EX2_18 : &'static str =
    "plain:\n  This unquoted scalar\n  spans many lines.\n\nquoted: \"So does this\n  quoted scalar.\\n\"";

// TODO: 2.19 - 2.22 schema tags

const EX2_23 : &'static str =
    "---\nnot-date: !!str 2002-04-28\n\npicture: !!binary |\n R0lGODlhDAAMAIQAAP//9/X\n 17unp5WZmZgAAAOfn515eXv\n Pz7Y6OjuDg4J+fn5OTk6enp\n 56enmleECcgggoBADs=\n\napplication specific tag: !something |\n The semantics of the tag\n above may be different for\n different documents.";

const EX2_24 : &'static str =
    "%TAG ! tag:clarkevans.com,2002:\n--- !shape\n  # Use the ! handle for presenting\n  # tag:clarkevans.com,2002:circle\n- !circle\n  center: &ORIGIN {x: 73, y: 129}\n  radius: 7\n- !line\n  start: *ORIGIN\n  finish: { x: 89, y: 102 }\n- !label\n  start: *ORIGIN\n  color: 0xFFEEBB\n  text: Pretty vector drawing.";

const EX2_25 : &'static str =
    "# Sets are represented as a\n# Mapping where each key is\n# associated with a null value\n--- !!set\n? Mark McGwire\n? Sammy Sosa\n? Ken Griffey";

const EX2_26 : &'static str =
    "# Ordered maps are represented as\n# A sequence of mappings, with\n# each mapping having one key\n--- !!omap\n- Mark McGwire: 65\n- Sammy Sosa: 63\n- Ken Griffey: 58";

const EX2_27 : &'static str =
    "--- !<tag:clarkevans.com,2002:invoice>\ninvoice: 34843\ndate   : 2001-01-23\nbill-to: &id001\n    given  : Chris\n    family : Dumars\n    address:\n        lines: |\n            458 Walkman Dr.\n            Suite #292\n        city    : Royal Oak\n        state   : MI\n        postal  : 48046\nship-to: *id001\nproduct:\n    - sku         : BL394D\n      quantity    : 4\n      description : Basketball\n      price       : 450.00\n    - sku         : BL4438H\n      quantity    : 1\n      description : Super Hoop\n      price       : 2392.00\ntax  : 251.42\ntotal: 4443.52\ncomments:\n    Late afternoon is best.\n    Backup contact is Nancy\n    Billsmer @ 338-4338.";

const EX2_28 : &'static str =
    "---\nTime: 2001-11-23 15:01:42 -5\nUser: ed\nWarning:\n  This is an error message\n  for the log file\n---\nTime: 2001-11-23 15:02:31 -5\nUser: ed\nWarning:\n  A slightly different error\n  message.\n---\nDate: 2001-11-23 15:03:17 -5\nUser: ed\nFatal:\n  Unknown variable \"bar\"\nStack:\n  - file: TopClass.py\n    line: 23\n    code: |\n      x = MoreObject(\"345\\n\")\n  - file: MoreClass.py\n    line: 58\n    code: |-\n      foo = bar";

// TODO: 5.1 - 5.2 BOM

const EX5_3 : &'static str =
    "sequence:\n- one\n- two\nmapping:\n  ? sky\n  : blue\n  sea : green";

const EX5_4 : &'static str =
    "sequence: [ one, two, ]\nmapping: { sky: blue, sea: green }";

const EX5_5 : &'static str = "# Comment only.";

const EX5_6 : &'static str =
    "anchored: !local &anchor value\nalias: *anchor";

const EX5_7 : &'static str =
    "literal: |\n  some\n  text\nfolded: >\n  some\n  text\n";

const EX5_8 : &'static str =
    "single: 'text'\ndouble: \"text\"";

// TODO: 5.9 directive
// TODO: 5.10 reserved indicator

const EX5_11 : &'static str =
    "|\n  Line break (no glyph)\n  Line break (glyphed)\n";

const EX5_12 : &'static str =
    "# Tabs and spaces\nquoted: \"Quoted\t\"\nblock:	|\n  void main() {\n  \tprintf(\"Hello, world!\\n\");\n  }";

const EX5_13 : &'static str =
    "\"Fun with \\\\\n\\\" \\a \\b \\e \\f \\\n\\n \\r \\t \\v \\0 \\\n\\  \\_ \\N \\L \\P \\\n\\x41 \\u0041 \\U00000041\"";

const EX5_14 : &'static str =
    "Bad escapes:\n  \"\\c\n  \\xq-\"";

const EX6_1 : &'static str =
    "  # Leading comment line spaces are\n   # neither content nor indentation.\n    \nNot indented:\n By one space: |\n    By four\n      spaces\n Flow style: [    # Leading spaces\n   By two,        # in flow style\n  Also by two,    # are neither\n  \tStill by two   # content nor\n    ]             # indentation.";

const EX6_2 : &'static str =
    "? a\n: -\tb\n  -  -\tc\n     - d";

const EX6_3 : &'static str =
    "- foo:\t bar\n- - baz\n  -\tbaz";

const EX6_4 : &'static str =
    "plain: text\n  lines\nquoted: \"text\n  \tlines\"\nblock: |\n  text\n   \tlines\n";

const EX6_5 : &'static str =
    "Folding:\n  \"Empty line\n   \t\n  as a line feed\"\nChomping: |\n  Clipped empty lines\n ";

const EX6_6 : &'static str =
    ">-\n  trimmed\n  \n \n\n  as\n  space";

const EX6_7 : &'static str =
    ">\n  foo \n \n  \t bar\n\n  baz\n";

const EX6_8 : &'static str =
    "\"\n  foo \n \n  \t bar\n\n  baz\n\"";

const EX6_9 : &'static str =
    "key:    # Comment\n  value";

const EX6_10 : &'static str =
    "  # Comment\n   \n\n";

const EX6_11 : &'static str =
    "key:    # Comment\n        # lines\n  value\n\n";

const EX6_12 : &'static str =
    "{ first: Sammy, last: Sosa }:\n# Statistics:\n  hr:  # Home runs\n     65\n  avg: # Average\n   0.278";

const EX6_13 : &'static str =
    "%FOO  bar baz # Should be ignored\n               # with a warning.\n--- \"foo\"";

const EX6_14 : &'static str =
    "%YAML 1.3 # Attempt parsing\n           # with a warning\n---\n\"foo\"";

const EX6_15 : &'static str =
    "%YAML 1.2\n%YAML 1.1\nfoo";

const EX6_16 : &'static str =
    "%TAG !yaml! tag:yaml.org,2002:\n---\n!yaml!str \"foo\"";

const EX6_17 : &'static str =
    "%TAG ! !foo\n%TAG ! !foo\nbar";

const EX6_18 : &'static str =
    "# Private\n!foo \"bar\"\n...\n# Global\n%TAG ! tag:example.com,2000:app/\n---\n!foo \"bar\"";

const EX6_19 : &'static str =
    "%TAG !! tag:example.com,2000:app/\n---\n!!int 1 - 3 # Interval, not integer";

const EX6_20 : &'static str =
    "%TAG !e! tag:example.com,2000:app/\n---\n!e!foo \"bar\"";

const EX6_21 : &'static str =
    "%TAG !m! !my-\n--- # Bulb here\n!m!light fluorescent\n...\n%TAG !m! !my-\n--- # Color here\n!m!light green";

const EX6_22 : &'static str =
    "%TAG !e! tag:example.com,2000:app/\n---\n- !e!foo \"bar\"";

const EX6_23 : &'static str =
    "!!str &a1 \"foo\":\n  !!str bar\n&a2 baz : *a1";

const EX6_24 : &'static str =
    "!<tag:yaml.org,2002:str> foo :\n  !<!bar> baz";

const EX6_25 : &'static str =
    "- !<!> foo\n- !<$:?> bar\n";

const EX6_26 : &'static str =
    "%TAG !e! tag:example.com,2000:app/\n---\n- !local foo\n- !!str bar\n- !e!tag%21 baz\n";

const EX6_27a : &'static str =
    "%TAG !e! tag:example,2000:app/\n---\n- !e! foo";

const EX6_27b : &'static str =
    "%TAG !e! tag:example,2000:app/\n---\n- !h!bar baz";

const EX6_28 : &'static str =
    "# Assuming conventional resolution:\n- \"12\"\n- 12\n- ! 12";

const EX6_29 : &'static str =
    "First occurrence: &anchor Value\nSecond occurrence: *anchor";

const EX7_1 : &'static str =
    "First occurrence: &anchor Foo\nSecond occurrence: *anchor\nOverride anchor: &anchor Bar\nReuse anchor: *anchor";

const EX7_2 : &'static str =
    "{\n  foo : !!str,\n  !!str : bar,\n}";

const EX7_3 : &'static str =
    "{\n  ? foo :,\n  : bar,\n}\n";

const EX7_4 : &'static str =
    "\"implicit block key\" : [\n  \"implicit flow key\" : value,\n ]";

const EX7_5 : &'static str =
    "\"folded \nto a space,\t\n \nto a line feed, or \t\\\n \\ \tnon-content\"";

const EX7_6 : &'static str =
    "\" 1st non-empty\n\n 2nd non-empty \n\t3rd non-empty \"";

const EX7_7 : &'static str = " 'here''s to \"quotes\"'";

const EX7_8 : &'static str =
    "'implicit block key' : [\n  'implicit flow key' : value,\n ]";

const EX7_9 : &'static str =
    "' 1st non-empty\n\n 2nd non-empty \n\t3rd non-empty '";

const EX7_10 : &'static str =
    "# Outside flow collection:\n- ::vector\n- \": - ()\"\n- Up, up, and away!\n- -123\n- http://example.com/foo#bar\n# Inside flow collection:\n- [ ::vector,\n  \": - ()\",\n  \"Up, up, and away!\",\n  -123,\n  http://example.com/foo#bar ]";

const EX7_11 : &'static str =
    "implicit block key : [\n  implicit flow key : value,\n ]";

const EX7_12 : &'static str =
    "1st non-empty\n\n 2nd non-empty \n\t3rd non-empty";

const EX7_13 : &'static str =
    "- [ one, two, ]\n- [three ,four]";

const EX7_14 : &'static str =
    "[\n\"double\n quoted\", 'single\n           quoted',\nplain\n text, [ nested ],\nsingle: pair,\n]";

const EX7_15 : &'static str =
    "- { one : two , three: four , }\n- {five: six,seven : eight}";

const EX7_16 : &'static str =
    "{\n? explicit: entry,\nimplicit: entry,\n?\n}";

const EX7_17 : &'static str =
    "{\nunquoted : \"separate\",\nhttp://foo.com,\nomitted value:,\n: omitted key,\n}";

const EX7_18 : &'static str =
    "{\n\"adjacent\":value,\n\"readable\":value,\n\"empty\":\n}";

const EX7_19 : &'static str =
    "[\nfoo: bar\n]";

const EX7_20 : &'static str =
    "[\n? foo\n bar : baz\n]";

const EX7_21 : &'static str =
    "- [ YAML : separate ]\n- [ : empty key entry ]\n- [ {JSON: like}:adjacent ]";

const EX7_22 : &'static str =
    "[ foo\n bar: invalid,";  // Note: we don't check (on purpose) the >1K chars for an
                       // implicit key

const EX7_23 : &'static str =
    "- [ a, b ]\n- { a: b }\n- \"a\"\n- 'b'\n- c";

const EX7_24 : &'static str =
    "- !!str \"a\"\n- 'b'\n- &anchor \"c\"\n- *anchor\n- !!str";

const EX8_1 : &'static str =
    "- | # Empty header\n literal\n- >1 # Indentation indicator\n  folded\n- |+ # Chomping indicator\n keep\n\n- >1- # Both indicators\n  strip\n";

const EX8_2 : &'static str =
    "- |\n detected\n- >\n \n  \n  # detected\n- |1\n  explicit\n- >\n \t\n detected\n";

const EX8_3a : &'static str =
    "- |\n  \n text";

const EX8_3b : &'static str =
    "- >\n  text\n text";

const EX8_3c : &'static str =
    "- |2\n text";

const EX8_4 : &'static str =
    "strip: |-\n  text\nclip: |\n  text\nkeep: |+\n  text\n";

const EX8_5 : &'static str =
    " # Strip\n  # Comments:\nstrip: |-\n  # text\n  \n # Clip\n  # comments:\n\nclip: |\n  # text\n \n # Keep\n  # comments:\n\nkeep: |+\n  # text\n\n # Trail\n  # Comments\n";

const EX8_6 : &'static str =
    "strip: >-\n\nclip: >\n\nkeep: |+\n\n";

const EX8_7 : &'static str =
    "|\n literal\n \ttext\n\n";

const EX8_8 : &'static str =
    "|\n \n  \n  literal\n   \n  \n  text\n\n # Comment\n";

const EX8_9 : &'static str =
    ">\n folded\n text\n\n";

const EX8_10 : &'static str =
    ">\n\n folded\n line\n\n next\n line\n   * bullet\n\n   * list\n   * lines\n\n last\n line\n\n# Comment\n";

const EX8_11 : &'static str = EX8_10;
const EX8_12 : &'static str = EX8_10;
const EX8_13 : &'static str = EX8_10;

const EX8_14 : &'static str =
    "block sequence:\n  - one\n  - two : three\n";

const EX8_15 : &'static str =
    "- # Empty\n- |\n block node\n- - one # Compact\n  - two # sequence\n- one: two # Compact mapping\n";

const EX8_16 : &'static str =
    "block mapping:\n key: value\n";

const EX8_17 : &'static str =
    "? explicit key # Empty value\n? |\n  block key\n: - one # Explicit compact\n  - two # block value\n";

// XXX libyaml failed this test
const EX8_18 : &'static str =
    "plain key: in-line value\n:  # Both empty\n\"quoted key\":\n- entry\n";

const EX8_19 : &'static str =
    "- sun: yellow\n- ? earth: blue\n  : moon: white\n";

const EX8_20 : &'static str =
    "-\n  \"flow in block\"\n- >\n Block scalar\n- !!map # Block collection\n  foo : bar\n";

const EX8_21 : &'static str =
    "literal: |2\n  value\nfolded:\n   !foo\n  >1\n value\n";

const EX8_22 : &'static str =
    "sequence: !!seq\n- entry\n- !!seq\n - nested\nmapping: !!map\n foo: bar\n";
