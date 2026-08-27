#!/usr/bin/env bats

setup() {
  PROMPT_COMMAND=''        # in case the invoking shell has set this
  history -s fake command  # preexec requires there be some history
  set -o nounset           # in case the user has this set
  __bp_delay_install="true"
  source "${BATS_TEST_DIRNAME}/../bash-preexec.sh"
}

# Evaluates all the elements of PROMPT_COMMAND
eval_PROMPT_COMMAND() {
  local lastexit=$? lastarg=$_ prompt_command
  for prompt_command in "${PROMPT_COMMAND[@]}"; do
    __bp_set_ret_value "$lastexit" "$lastarg"
    eval "$prompt_command"
  done
}

# Joins the elements of PROMPT_COMMAND with $'\n'
join_PROMPT_COMMAND() {
  local IFS=$'\n'
  echo "${PROMPT_COMMAND[*]}"
}

bp_install() {
  __bp_install_after_session_init
  eval_PROMPT_COMMAND
}

test_echo() {
  echo "test echo"
}

test_preexec_echo() {
  printf "%s\n" "$1"
}

# Helper functions necessary because Bats' run doesn't preserve $?
return_exit_code() {
  return $1
}

set_exit_code_and_run_precmd() {
  return_exit_code "${1:-0}" "${2-$_}"
  __bp_precmd_invoke_cmd
}


@test "sourcing bash-preexec should exit with 1 if we're not using bash" {
  unset BASH_VERSION
  run source "${BATS_TEST_DIRNAME}/../bash-preexec.sh"
  [ $status -eq 1 ]
  [ -z "$output" ]
}

@test "sourcing bash-preexec should exit with 1 if we're using an older version of bash" {
  if type -p bash-3.0 &>/dev/null; then
    run bash-3.0 -c "source \"${BATS_TEST_DIRNAME}/../bash-preexec.sh\""
    [ "$status" -eq 1 ]
    [ -z "$output" ]
  else
    skip
  fi
}

@test "__bp_install should exit if it's already installed" {
  bp_install

  run '__bp_install'
  [ $status -eq 1 ]
  [ -z "$output" ]
}

@test "__bp_install should remove trap logic and itself from PROMPT_COMMAND" {
  __bp_install_after_session_init

  # Assert that before running, the command contains the install string, and
  # afterwards it does not
  [[ "$(join_PROMPT_COMMAND)" == *"$__bp_install_string"* ]] || return 1

  eval_PROMPT_COMMAND

  [[ "$(join_PROMPT_COMMAND)" != *"$__bp_install_string"* ]] || return 1
}

@test "__bp_install should remove trap logic and itself from modified PROMPT_COMMAND" {
  PROMPT_COMMAND=()
  __bp_install_after_session_init
  PROMPT_COMMAND="$PROMPT_COMMAND; true"

  # Assert that before running, the command contains the install string, and
  # afterwards it does not
  [[ "$(join_PROMPT_COMMAND)" == *"$__bp_install_string"* ]] || return 1

  eval_PROMPT_COMMAND

  [[ "$(join_PROMPT_COMMAND)" != *"$__bp_install_string"* ]] || return 1
}


@test "__bp_install should preserve an existing DEBUG trap" {
  trap_invoked_count=0
  foo() { (( trap_invoked_count += 1 )); }

  # note setting this causes BATS to mis-report the failure line when this test fails
  trap foo DEBUG
  original_debug_trap=$(trap -p DEBUG)
  [ "$(cut -d' ' -f3 <<< "$original_debug_trap")" == "'foo'" ]

  bp_install
  trap_count_snapshot=$trap_invoked_count

  if [[ $__bp_hook_preexec_proc == '__bp_hook_preexec_into_debug' ]]; then
    # We override the DEBUG trap with the DEBUG approach to preexec
    [ "$(trap -p DEBUG | cut -d' ' -f3)" == "'__bp_preexec_invoke_exec" ]
    [[ "${preexec_functions[*]}" == *"__bp_original_debug_trap"* ]] || return 1
    [[ $(declare -f __bp_original_debug_trap) == *$'\n'"    foo"$'\n'* ]] || return 1
  else
    # We do not modify the DEBUG trap in the other approaches
    [ "$(trap -p DEBUG)" == "$original_debug_trap" ]
  fi

  __bp_interactive_mode # triggers the DEBUG trap

  # ensure the trap count is still being incremented after the trap's been overwritten
  (( trap_count_snapshot < trap_invoked_count ))
}

@test "__bp_install should preserve an existing DEBUG trap containing quotes" {
  trap_invoked_count=0
  foo() { (( trap_invoked_count += 1 )); }

  # constants
  single_quote="'" single_quote_escape="'\''"
  original_trap_command="foo && echo 'hello' > /dev/null"

  # note setting this causes BATS to mis-report the failure line when this test fails
  trap "$original_trap_command" debug
  original_debug_trap=$(trap -p DEBUG)
  [ "$original_debug_trap" == "trap -- '${original_trap_command//$single_quote/$single_quote_escape}' DEBUG" ]

  bp_install
  trap_count_snapshot=$trap_invoked_count

  if [[ $__bp_hook_preexec_proc == '__bp_hook_preexec_into_debug' ]]; then
    # We override the DEBUG trap with the DEBUG approach to preexec
    [ "$(trap -p DEBUG | cut -d' ' -f3)" == "'__bp_preexec_invoke_exec" ]
    [[ "${preexec_functions[*]}" == *"__bp_original_debug_trap"* ]] || return 1
    [[ $(declare -f __bp_original_debug_trap) == *$'\n'"    $original_trap_command"$'\n'* ]] || return 1
  else
    # We do not modify the DEBUG trap in the other approaches
    [ "$(trap -p DEBUG)" == "$original_debug_trap" ]
  fi

  __bp_interactive_mode # triggers the DEBUG trap

  # ensure the trap count is still being incremented after the trap's been overwritten
  (( trap_count_snapshot < trap_invoked_count ))
}

@test "__bp_install should preserve an existing PS0" {
  original_PS0=${PS0-}

  bp_install

  if [[ $__bp_hook_preexec_proc == '__bp_hook_preexec_into_ps0' ]]; then
    # We modify PS0 with the PS0 approach to preexec, but the original contents
    # of PS0 should be preserved.
    [ "${PS0-}" != "$original_PS0" ]
    [[ ${PS0-} == *"$original_PS0"* ]] || return 1
  else
    # We do not modify PS0 in the other approaches
    [ "${PS0-}" == "$original_PS0" ]
  fi
}

@test "__bp_install_prompt_command should adjust modified PROMPT_COMMAND" {
    unset -v PROMPT_COMMAND
    PROMPT_COMMAND="echo PREHOOK"

    # First install
    __bp_install_prompt_command
    expected_result=$'__bp_precmd_invoke_cmd "$_"\necho PREHOOK\n__bp_interactive_mode'
    [ "$(join_PROMPT_COMMAND)" == "$expected_result" ]

    # User modification
    if __bp_use_array_prompt_command; then
        PROMPT_COMMAND+=('echo POSTHOOK')
    else
        PROMPT_COMMAND+=$'\necho POSTHOOK'
    fi
    expected_result=$'__bp_precmd_invoke_cmd "$_"\necho PREHOOK\n__bp_interactive_mode\necho POSTHOOK'
    [ "$(join_PROMPT_COMMAND)" == "$expected_result" ]

    # Re-adjust
    __bp_install_prompt_command
    expected_result=$'__bp_precmd_invoke_cmd "$_"\necho PREHOOK\necho POSTHOOK\n__bp_interactive_mode'
    [ "$(join_PROMPT_COMMAND)" == "$expected_result" ]
}

@test "__bp_install_prompt_command should be skipped when already set up" {
    unset -v PROMPT_COMMAND
    PROMPT_COMMAND=""

    # First install should succeed
    __bp_install_prompt_command || return 1

    # Second install should skip processing and return 1
    ! __bp_install_prompt_command || return 1
}

@test "__bp_sanitize_string should remove semicolons and trim space" {

    __bp_sanitize_string output "   true1;  "$'\n'
    [ "$output" == "true1" ]

    __bp_sanitize_string output " ; true2;  "
    [ "$output" == "true2" ]

    __bp_sanitize_string output $'\n'" ; true3;  "
    [ "$output" == "true3" ]

}

@test "__bp_sanitize_string should remove no-op colons" {
    __bp_sanitize_string output ':'
    [ "$output" == "" ]

    __bp_sanitize_string output $':\n:'
    [ "$output" == "" ]

    __bp_sanitize_string output $':\n:;echo USER1'
    [ "$output" == "echo USER1" ]

    __bp_sanitize_string output $'echo USER2\n:\necho USER3'
    expected_result=$'echo USER2\necho USER3'
    [ "$output" == "$expected_result" ]

    __bp_sanitize_string output $'echo USER4;:;echo USER5'
    expected_result=$'echo USER4\necho USER5'
    [ "$output" == "$expected_result" ]

    __bp_sanitize_string output $'echo USER6;:\necho USER7'
    expected_result=$'echo USER6\necho USER7'
    [ "$output" == "$expected_result" ]

    __bp_sanitize_string output $':\n: ; echo USER8'
    [ "$output" == "echo USER8" ]

    __bp_sanitize_string output $':\n:  ;  echo USER9'
    [ "$output" == "echo USER9" ]

    __bp_sanitize_string output $'echo USER10 ; :\n: ; echo USER11'
    expected_result=$'echo USER10\necho USER11'
    [ "$output" == "$expected_result" ]
}

@test "Appending to PROMPT_COMMAND should work after bp_install" {
    bp_install

    PROMPT_COMMAND="$PROMPT_COMMAND; true"
    eval_PROMPT_COMMAND
}

@test "Appending or prepending to PROMPT_COMMAND should work after bp_install_after_session_init" {
    __bp_install_after_session_init
    nl=$'\n'
    # On Bash 5.1+ we append to the array properly,
    # so first element of PROMPT_COMMAND may still be empty.
    PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND; }true"
    PROMPT_COMMAND="$PROMPT_COMMAND $nl true"
    PROMPT_COMMAND="$PROMPT_COMMAND; true"
    PROMPT_COMMAND="true; $PROMPT_COMMAND"
    PROMPT_COMMAND="true; $PROMPT_COMMAND"
    PROMPT_COMMAND="true; $PROMPT_COMMAND"
    PROMPT_COMMAND="true $nl $PROMPT_COMMAND"
    eval_PROMPT_COMMAND
}

@test "Appending or prepending to PROMPT_COMMAND array should work after bp_install_after_session_init" {
    if __bp_use_array_prompt_command; then
        PROMPT_COMMAND=()
        __bp_install_after_session_init
        nl=$'\n'
        PROMPT_COMMAND=("${PROMPT_COMMAND[@]}" "true")
        PROMPT_COMMAND=("${PROMPT_COMMAND[@]}" "$nl true")
        PROMPT_COMMAND=("${PROMPT_COMMAND[@]}" "true")
        PROMPT_COMMAND=("true" "${PROMPT_COMMAND[@]}")
        PROMPT_COMMAND=("true" "${PROMPT_COMMAND[@]}")
        PROMPT_COMMAND=("true" "${PROMPT_COMMAND[@]}")
        PROMPT_COMMAND=("true $nl" "${PROMPT_COMMAND[@]}")
        eval_PROMPT_COMMAND
    else
        skip
    fi
}

# Case where a user is appending or prepending to PROMPT_COMMAND.
# This can happen after 'source bash-preexec.sh' e.g.
# source bash-preexec.sh; PROMPT_COMMAND="$PROMPT_COMMAND; other_prompt_command_hook"
@test "Adding to PROMPT_COMMAND before and after initiating install" {
    PROMPT_COMMAND="echo before"
    PROMPT_COMMAND="$PROMPT_COMMAND; echo before2"
    __bp_install_after_session_init
    PROMPT_COMMAND="$PROMPT_COMMAND"$'\necho after'
    PROMPT_COMMAND="echo after2; $PROMPT_COMMAND;"

    eval_PROMPT_COMMAND

    expected_result=$'__bp_precmd_invoke_cmd "$_"\necho after2; echo before; echo before2\necho after\n__bp_interactive_mode'
    [ "$(join_PROMPT_COMMAND)" == "$expected_result" ]
}

@test "Adding to PROMPT_COMMAND after with semicolon" {
    PROMPT_COMMAND="echo before"
    __bp_install_after_session_init
    if __bp_use_array_prompt_command; then
        PROMPT_COMMAND[${#PROMPT_COMMAND[@]}-1]+='; echo after'
    else
        PROMPT_COMMAND+='; echo after'
    fi

    eval_PROMPT_COMMAND

    expected_result=$'__bp_precmd_invoke_cmd "$_"\necho before\necho after\n__bp_interactive_mode'
    [ "$(join_PROMPT_COMMAND)" == "$expected_result" ]
}

@test "during install PROMPT_COMMAND and precmd functions should be executed each once" {
    PROMPT_COMMAND="echo before"
    PROMPT_COMMAND="$PROMPT_COMMAND; echo before2"
    __bp_install_after_session_init
    PROMPT_COMMAND="$PROMPT_COMMAND; echo after"
    PROMPT_COMMAND="echo after2; $PROMPT_COMMAND;"

    precmd() { echo "inside precmd"; }
    run eval_PROMPT_COMMAND
    [ "${#lines[@]}" == '5' ]
    [ "${lines[0]}" == "after2" ]
    [ "${lines[1]}" == "before" ]
    [ "${lines[2]}" == "before2" ]
    if __bp_use_array_prompt_command; then
        [ "${lines[3]}" == "after" ]
        [ "${lines[4]}" == "inside precmd" ]
    else
        [ "${lines[3]}" == "inside precmd" ]
        [ "${lines[4]}" == "after" ]
    fi
}

@test "during install PROMPT_COMMAND and precmd functions should be executed each once (Bash 5.1+ PROMPT_COMMAND array)" {
    if __bp_use_array_prompt_command; then
        PROMPT_COMMAND=("echo before")
        PROMPT_COMMAND=("${PROMPT_COMMAND[@]}" "echo before2")
        __bp_install_after_session_init
        PROMPT_COMMAND=("${PROMPT_COMMAND[@]}" "echo after")
        PROMPT_COMMAND=("echo after2" "${PROMPT_COMMAND[@]}")

        precmd() { echo "inside precmd"; }
        run eval_PROMPT_COMMAND
        [ "${#lines[@]}" == '5' ]
        [ "${lines[0]}" == "after2" ]
        [ "${lines[1]}" == "before" ]
        [ "${lines[2]}" == "before2" ]
        [ "${lines[3]}" == "inside precmd" ]
        [ "${lines[4]}" == "after" ]
    else
        skip
    fi
}

@test "No functions defined for preexec should simply return" {
    __bp_interactive_mode

    run '__bp_preexec_invoke_exec' 'true'
    [ $status -eq 0 ]
    [ -z "$output" ]
}

@test "precmd should execute a function once" {
    precmd_functions+=(test_echo)
    run set_exit_code_and_run_precmd
    [ $status -eq 0 ]
    [ "$output" == "test echo" ]
}

@test "precmd should set \$? to be the previous exit code" {
    echo_exit_code() {
      echo "$?"
    }

    precmd_functions+=(echo_exit_code)
    run set_exit_code_and_run_precmd 251
    [ $status -eq 251 ]
    [ "$output" == "251" ]
}

@test "__bp_precmd_invoke_cmd should preserve \$? when taking the reentry-guard early return" {
    # When (( __bp_inside_precmd > 0 )) is true, __bp_precmd_invoke_cmd returns
    # early without reaching the trailing __bp_set_ret_value call.  Without the
    # explicit set_ret_value on this path, the bare `return` propagates the
    # exit status of the (( ... )) test (always 0 when the condition is true),
    # clobbering the user's actual last exit status.
    run_with_reentry_guard() {
        local __bp_inside_precmd=1
        return_exit_code 7
        __bp_precmd_invoke_cmd
    }
    run run_with_reentry_guard
    [ $status -eq 7 ]
}

@test "__bp_precmd_invoke_cmd should preserve \$? when taking the nested-call-frame early return" {
    # Once PROMPT_COMMAND already contains our hooks,
    # __bp_install_prompt_command returns 1, and if __bp_precmd_invoke_cmd is
    # invoked from inside another function (FUNCNAME depth > 1), the function
    # takes the "another framework wrapped our PROMPT_COMMAND" early
    # return. Without the explicit set_ret_value on this path the function used
    # to `return 0` and clobbered the caller's last exit status.
    #
    # The path's own guard short-circuits when BATS_VERSION is set so bats's
    # own scaffolding doesn't trip it; we clear BATS_VERSION inside the nested
    # call to actually exercise the path.
    bp_install
    run_nested() {
        BATS_VERSION="" return_exit_code 7
        BATS_VERSION="" __bp_precmd_invoke_cmd
    }
    run run_nested
    [ $status -eq 7 ]
}

@test "__bp_precmd_invoke_cmd installed in PROMPT_COMMAND should preserve \$? and \$_" {
    unset -v PROMPT_COMMAND
    PROMPT_COMMAND='save_lastexit=$? save_lastarg=$_'

    __bp_install_prompt_command
    precmd_functions=(precmd)

    # Note: The DEBUG and ERR traps set by Bats overwrite $_, so we cannot
    # properly test the values of $_.  We modify the DEBUG trap of Bats so that
    # it properly preserves the value of $_.  When we are not sure that the
    # current DEBUG trap preserves $_, we skip the test.  See
    # https://github.com/bats-core/bats-core/pull/1208 for details.
    if trap -p DEBUG | grep -qF \''bats_debug_trap "$BASH_SOURCE"'\'; then
        bats_debug_trap_modified=1
        trap -- 'bats_debug_trap "$BASH_SOURCE" "$_"' DEBUG
    fi
    if ! trap -p DEBUG | grep -qF ' "$_"'\'; then
        # If Bats' DEBUG trap does not preserve $_, and if it is not
        # successfully updated to include "$_", we skip the test.
        skip
    elif [[ ${bats_debug_trap_modified-} && BASH_VERSINFO[0] -le 3 ]]; then
        # When Bats' DEBUG trap does not care about $_, even if we modify the
        # DEBUG trap, an issue still seems to remain in Bash 3.2.  Therefore,
        # we skip the test in Bash 3.2 when Bat's DEBUG trap is modified.
        skip
    else
        run_prompt_command() {
            __bp_set_ret_value '77' 'lastarg!bzBtcQDLoM'
            eval_PROMPT_COMMAND
        }
        run_prompt_command || true
        [ "$save_lastexit" == '77' ]
        [ "$save_lastarg" == 'lastarg!bzBtcQDLoM' ]
    fi
}

@test "precmd should set \$BP_PIPESTATUS to the previous \$PIPESTATUS" {
  echo_pipestatus() {
    echo "${BP_PIPESTATUS[*]}"
  }
  # Helper function is necessary because Bats' run doesn't preserve $PIPESTATUS
  set_pipestatus_and_run_precmd() {
    false | true
    __bp_precmd_invoke_cmd
  }

  precmd_functions+=(echo_pipestatus)
  run 'set_pipestatus_and_run_precmd'
  [ $status -eq 0 ]
  [ "$output" == "1 0" ]
}

@test "precmd should set \$_ to be the previous last arg" {
    echo_last_arg() {
      echo "$_"
    }
    precmd_functions+=(echo_last_arg)

    bats_trap=$(trap -p DEBUG)
    trap DEBUG # remove the Bats stack-trace trap so $_ doesn't get overwritten
    : "last-arg"
    __bp_preexec_interactive_mode=1 __bp_preexec_invoke_exec "$_"
    eval "$bats_trap" # Restore trap
    run set_exit_code_and_run_precmd 0 "$__bp_last_argument_prev_command"
    [ $status -eq 0 ]
    [ "$output" == "last-arg" ]
}

@test "preexec should execute a function with the last command in our history" {
    preexec_functions+=(test_preexec_echo)
    __bp_interactive_mode
    git_command="git commit -a -m 'committing some stuff'"
    history -s $git_command

    run '__bp_preexec_invoke_exec'
    [ $status -eq 0 ]
    [ "$output" == "$git_command" ]
}

@test "preexec should execute multiple functions in the order added to their arrays" {
    fun_1() { echo "$1 one"; }
    fun_2() { echo "$1 two"; }
    preexec_functions+=(fun_1)
    preexec_functions+=(fun_2)
    __bp_interactive_mode

    run '__bp_preexec_invoke_exec'
    [ $status -eq 0 ]
    [ "${#lines[@]}" == '2' ]
    [ "${lines[0]}" == "fake command one" ]
    [ "${lines[1]}" == "fake command two" ]
}

@test "preexec_functions before initialization should be preserved" {
    fun_1() { reply_1="$1 one"; }
    fun_2() { reply_2="$1 two"; }

    # preexec registered before initialization
    preexec_functions+=(fun_1)

    # Initialization
    __bp_preexec_interactive_mode="" bp_install

    # preexec registered after initialization
    preexec_functions+=(fun_2)

    reply_1=""
    reply_2=""
    __bp_invoke_preexec_from_ps0 || return 1
    [ "$reply_1" == "fake command one" ]
    [ "$reply_2" == "fake command two" ]
}

@test "precmd should execute multiple functions in the order added to their arrays" {
    fun_1() { echo "one"; }
    fun_2() { echo "two"; }
    precmd_functions+=(fun_1)
    precmd_functions+=(fun_2)

    run set_exit_code_and_run_precmd
    [ $status -eq 0 ]
    [ "${#lines[@]}" == '2' ]
    [ "${lines[0]}" == "one" ]
    [ "${lines[1]}" == "two" ]
}

@test "preexec should execute a function with IFS defined to local scope" {
    IFS=_
    name_with_underscores_1() { parts=(1_2); echo $parts; }
    preexec_functions+=(name_with_underscores_1)

    __bp_interactive_mode
    run '__bp_preexec_invoke_exec'
    [ $status -eq 0 ]
    [ "$output" == "1 2" ]
}

@test "precmd should execute a function with IFS defined to local scope" {
    IFS=_
    name_with_underscores_2() { parts=(2_2); echo $parts; }
    precmd_functions+=(name_with_underscores_2)
    run set_exit_code_and_run_precmd
    [ $status -eq 0 ]
    [ "$output" == "2 2" ]
}

@test "preexec should set \$? to be the exit code of preexec_functions" {
    return_nonzero() {
      return 1
    }
    preexec_functions+=(return_nonzero)

    __bp_interactive_mode

    run '__bp_preexec_invoke_exec'
    [ $status -eq 1 ]
}

@test "__bp_invoke_precmd_functions should be transparent for \$? and \$_" {
  tester1() { test1_lastexit=$? test1_lastarg=$_; }
  tester2() { test2_lastexit=$? test2_lastarg=$_; }
  precmd_functions=(tester1 tester2)
  trap - DEBUG # remove the Bats stack-trace trap so $_ doesn't get overwritten
  __bp_invoke_precmd_functions 111 'vxxJlwNx9VPJDA' || true

  [ "$test1_lastexit" == 111 ]
  [ "$test1_lastarg" == 'vxxJlwNx9VPJDA' ]
  [ "$test2_lastexit" == 111 ]
  [ "$test2_lastarg" == 'vxxJlwNx9VPJDA' ]
}

@test "__bp_invoke_precmd_functions returns the last non-zero exit status" {
  tester1() { return 91; }
  tester2() { return 38; }
  tester3() { return 0; }
  precmd_functions=(tester1 tester2 tester3)
  status=0
  __bp_invoke_precmd_functions 1 'lastarg' || status=$?

  [ "$status" == 38 ]

  precmd_functions=(tester3)
  status=0
  __bp_invoke_precmd_functions 1 'lastarg' || status=$?

  [ "$status" == 0 ]
}

@test "__bp_invoke_preexec_functions should be transparent for \$? and \$_" {
  tester1() { test1_lastexit=$? test1_lastarg=$_; }
  tester2() { test2_lastexit=$? test2_lastarg=$_; }
  preexec_functions=(tester1 tester2)
  trap - DEBUG # remove the Bats stack-trace trap so $_ doesn't get overwritten
  __bp_invoke_preexec_functions 87 'ehQrzHTHtE2E7Q' 'command' || true

  [ "$test1_lastexit" == 87 ]
  [ "$test1_lastarg" == 'ehQrzHTHtE2E7Q' ]
  [ "$test2_lastexit" == 87 ]
  [ "$test2_lastarg" == 'ehQrzHTHtE2E7Q' ]
}

@test "__bp_invoke_preexec_functions returns the last non-zero exit status" {
  tester1() { return 52; }
  tester2() { return 112; }
  tester3() { return 0; }
  preexec_functions=(tester1 tester2 tester3)
  status=0
  __bp_invoke_preexec_functions 1 'lastarg' 'command' || status=$?

  [ "$status" == 112 ]

  preexec_functions=(tester3)
  status=0
  __bp_invoke_preexec_functions 1 'lastarg' 'command' || status=$?

  [ "$status" == 0 ]
}

@test "__bp_invoke_preexec_functions should supply a current command in the first argument" {
  tester1() { test1_bash_command=$1; }
  tester2() { test2_bash_command=$1; }
  preexec_functions=(tester1 tester2)
  __bp_invoke_preexec_functions 1 'lastarg' 'UEVkErELArSwjA' || true

  [ "$test1_bash_command" == 'UEVkErELArSwjA' ]
  [ "$test2_bash_command" == 'UEVkErELArSwjA' ]
}

@test "in_prompt_command should detect if a command is part of PROMPT_COMMAND" {

    PROMPT_COMMAND=$'precmd_invoke_cmd\n something; echo yo\n __bp_interactive_mode'
    run '__bp_in_prompt_command' "something"
    [ $status -eq 0 ]

    run '__bp_in_prompt_command' "something_else"
    [ $status -eq 1 ]

    # Should trim commands and arguments here.
    PROMPT_COMMAND=" precmd_invoke_cmd ; something ; some_stuff_here;"
    run '__bp_in_prompt_command' " precmd_invoke_cmd "
    [ $status -eq 0 ]

    PROMPT_COMMAND=" precmd_invoke_cmd ; something ; some_stuff_here;"
    run '__bp_in_prompt_command' " not_found"
    [ $status -eq 1 ]

}

@test "__bp_adjust_histcontrol should remove ignorespace and ignoreboth" {

    # Should remove ignorespace
    HISTCONTROL="ignorespace:ignoredups:*"
    __bp_adjust_histcontrol
    [ "$HISTCONTROL" == ":ignoredups:*" ]

    # Should remove ignoreboth and replace it with ignoredups
    HISTCONTROL="ignoreboth"
    __bp_adjust_histcontrol
    [ "$HISTCONTROL" == "ignoredups:" ]

    # Handle a few inputs
    HISTCONTROL="ignoreboth:ignorespace:some_thing_else"
    __bp_adjust_histcontrol
    echo "$HISTCONTROL"
    [ "$HISTCONTROL" == "ignoredups:::some_thing_else" ]

}

@test "preexec should respect HISTTIMEFORMAT" {
    preexec_functions+=(test_preexec_echo)
    __bp_interactive_mode
    git_command="git commit -a -m 'committing some stuff'"
    HISTTIMEFORMAT='%F %T '
    history -s $git_command

    run '__bp_preexec_invoke_exec'
    [ $status -eq 0 ]
    [ "$output" == "$git_command" ]
}

@test "preexec should not strip whitespace from commands" {
    preexec_functions+=(test_preexec_echo)
    __bp_interactive_mode
    history -s " this command has whitespace "

    run '__bp_preexec_invoke_exec'
    [ $status -eq 0 ]
    [ "$output" == " this command has whitespace " ]
}

@test "preexec should preserve multi-line strings in commands" {
    preexec_functions+=(test_preexec_echo)
    __bp_interactive_mode
    history -s "this 'command contains
a multiline string'"
    run '__bp_preexec_invoke_exec'
    [ $status -eq 0 ]
    [ "$output" == "this 'command contains
a multiline string'" ]
}

@test "preexec should work on options to 'echo' commands" {
    preexec_functions+=(test_preexec_echo)
    __bp_interactive_mode
    history -s -- '-n'
    run '__bp_preexec_invoke_exec'
    [ $status -eq 0 ]
    [ "$output" == '-n' ]
}
