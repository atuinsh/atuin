; PowerShell highlight queries, adjusted for Atuin.
; This tries to map colors to the PSReadLine categories, see the color properties in Get-PSReadLineOption,
; or the const fields in the PSConsoleReadLineOptions class.
; Reuses parts of the original tree-sitter-powershell queries

; Anonymous nodes can be collected with:
; jq -r '.[] | select(.named == false) | .type' ./src/node-types.json | Sort-Object -Unique


; Reset to base state

(sub_expression) @base
(array_expression) @base
(hash_literal_expression) @base
(script_block_expression) @base

(type_spec) @base

(member_access
  (member_name) @base)


; Keywords

[
  "begin"
  "break"
  "catch"
  "continue"
  "data"
  "do"
  "dynamicparam"
  "else"
  "elseif"
  "end"
  "exit"
  "filter"
  "finally"
  "for"
  "foreach"
  "function"
  "if"
  "in"
  "inlinescript"
  "parallel"
  "param"
  "process"
  "return"
  "sequence"
  "switch"
  "throw"
  "trap"
  "try"
  "until"
  "while"
  "workflow"
  ] @keyword


; Operators

[
  "." "="
  "&" "|"
  "&&" "||"
  "+" "-" "*" "/" "%"
  "++" "--"
  ".." "::"
  "-and" "-or" "-xor"
  "-band" "-bor" "-bxor"
  "-not" "-bnot"
  "-f" "--%"
  ] @operator

(assignement_operator) @operator
(file_redirection_operator) @operator
(merging_redirection_operator) @operator
(comparison_operator) @operator
(switch_parameter) @operator
(stop_parsing) @operator


; Literals

(string_literal) @string

(integer_literal) @number
(real_literal) @number


; Commands

(command
  command_name: [(command_name) (command_name_expr)] @command)

((command_parameter) @flag
  (#match? @flag "^-"))

(function_statement
  (function_name) @command)


; Other

(variable) @variable

(comment) @comment


; Best-effort error recovery for tree-sitter-powershell v0.26.4

; Flags ending with a number, e.g. in "rg -C5 foo", the 5 is not considered as part of the argument
((command_parameter) . ((redirection (file_redirection_operator (MISSING ">")) (redirected_file_name) @flag)))

; Everything breaks when a command contains a "--" argument
(ERROR (command_name) @command "--" @flag)
(ERROR (command_name) ("--" . (simple_name) @flag))
