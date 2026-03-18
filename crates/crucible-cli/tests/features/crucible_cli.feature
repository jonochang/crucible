Feature: Crucible CLI

  Scenario: Show version
    When I run version
    Then the version command prints the package version
    And the flag version prints the package version

  Scenario: Initialize config in a new project
    Given an empty temp project
    When I run config init
    Then the config file is created

  Scenario: Show review help
    When I run review help
    Then the review help shows usage

  Scenario: Run review with a mock agent
    Given a git repo with a diff
    And a mock crucible config
    When I run review
    Then the review verdict is pass
    And the review findings include the mock finding

  Scenario: Review emits progress output
    Given a git repo with a diff
    And a mock crucible config
    When I run review
    Then progress output is emitted
    And startup header is shown
    And startup phase output is shown
    And round status output includes durations
    And analysis section is shown
    And system context section is shown
    And convergence output is shown

  Scenario: Review exports issues with locations
    Given a git repo with a diff
    And a mock crucible config
    When I run review with issue export
    Then issues are exported with code locations

  Scenario: Review exports full report to file
    Given a git repo with a diff
    And a mock crucible config
    When I run review with report export
    Then the full report artifact is written
    And run-scoped artifacts are written

  Scenario: Review skips clean local target
    Given a clean git repo
    And a mock crucible config
    When I run review with local target
    Then the review reports no changes and exits successfully

  Scenario: Review rejects conflicting target modes
    Given a git repo with a diff
    And a mock crucible config
    When I run review with conflicting target modes
    Then the review fails because target modes conflict

  Scenario: Review rejects GitHub actions without a PR target
    Given a git repo with a diff
    And a mock crucible config
    When I run review with GitHub dry-run on local target
    Then the review fails because GitHub review requires a PR target

  Scenario: Review rejects conflicting GitHub actions
    Given a git repo with a diff
    And a mock crucible config
    When I run review with conflicting GitHub actions for PR
    Then the review fails because GitHub actions conflict

  Scenario: Review errors when repo target has no diff
    Given a clean git repo with remote main configured
    And a mock crucible config
    When I run review with repo target
    Then the review fails because repo target has no diff

  Scenario: Review errors when branch target has no diff
    Given a clean git repo with remote main configured
    And a mock crucible config
    When I run review with branch target main
    Then the review fails because branch target has no diff

  Scenario: Review errors when requested files have no diff
    Given a clean git repo
    And a mock crucible config
    When I run review with file target README.md
    Then the review fails because requested files have no diff

  Scenario: Review captures related references in the agent prompt
    Given a git repo with symbol references in nearby files
    And a prompt-capturing mock crucible config
    When I run review
    Then the captured prompt includes related reference snippets

  Scenario: Review generates a GitHub dry-run review
    Given a git repo with a diff
    And a mock crucible config
    And a mock GitHub CLI
    When I run PR review with GitHub dry-run
    Then the GitHub dry-run output includes inline comments
    And the report includes a structured PR review draft

  Scenario: Review publishes a GitHub PR review
    Given a git repo with a diff
    And a mock crucible config
    And a mock GitHub CLI
    When I run PR review and publish GitHub review
    Then the GitHub review payload is posted

  Scenario: Review maps deleted source ranges to GitHub left-side comments
    Given a git repo with a deleted source range diff
    And a deleted-range mock crucible config
    And a mock GitHub CLI
    When I run PR review with GitHub dry-run
    Then the report maps deleted ranges to left-side inline comments

  Scenario: Review continues when an agent returns malformed JSON
    Given a git repo with a diff
    And a malformed mock crucible config
    When I run review
    Then the review process completes successfully
    And the review verdict is warn
    And the final report includes the malformed agent failure

  Scenario: Review continues when an agent exits non-zero
    Given a git repo with a diff
    And a failing mock crucible config
    When I run review
    Then the review process completes successfully
    And the review verdict is warn
    And the final report includes the failed agent process

  Scenario: Review exits on Ctrl+C
    Given a git repo with a diff
    And a slow mock crucible config
    When I interrupt review
    Then the review exits with code 130

  Scenario: Review exits after completion by default
    Given a git repo with a diff
    And a mock crucible config
    When I run review
    Then the review process completes successfully

  @real-agents
  Scenario: Run review with real agents
    Given a git repo with a diff
    And a real agent crucible config
    When I run review
    Then the review output is valid
