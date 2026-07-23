#!/usr/bin/env bats

@test "should not import if it's already defined" {
  bash_preexec_imported="defined"
  source "${BATS_TEST_DIRNAME}/../bash-preexec.sh"
  [ -z $(type -t __bp_install) ]
}

@test "should not import if it's already defined (old guard, don't use elsewhere!)" {
  __bp_imported="defined"
  source "${BATS_TEST_DIRNAME}/../bash-preexec.sh"
  [ -z $(type -t __bp_install) ]
}

@test "should import if not defined" {
  unset bash_preexec_imported
  source "${BATS_TEST_DIRNAME}/../bash-preexec.sh"
  [ -n $(type -t __bp_install) ]
}

@test "bp should stop installation if HISTTIMEFORMAT is readonly" {
  readonly HISTTIMEFORMAT
  run source "${BATS_TEST_DIRNAME}/../bash-preexec.sh"
  [ $status -ne 0 ]
  [[ "$output" =~ "HISTTIMEFORMAT" ]] || return 1
}
