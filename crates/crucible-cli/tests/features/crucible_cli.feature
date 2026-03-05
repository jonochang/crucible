Feature: Crucible CLI

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

  Scenario: Review exports issues with locations
    Given a git repo with a diff
    And a mock crucible config
    When I run review with issue export
    Then issues are exported with code locations

  Scenario: Review exits on Ctrl+C
    Given a git repo with a diff
    And a slow mock crucible config
    When I interrupt review
    Then the review exits with code 130

  @real-agents
  Scenario: Run review with real agents
    Given a git repo with a diff
    And a real agent crucible config
    When I run review
    Then the review output is valid
