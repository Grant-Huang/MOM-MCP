# MOM-MCP delivery

Use the feature map and the acceptance checklist for each delivery milestone. Run the milestone gate and earlier regressions before publishing the stage. Once the tests pass, commit the stage on its own branch, push it, and open a pull request with the scope, test command, results, and any external prerequisites. Use the preceding stage branch as the base while that stage is under review. Use master once the preceding stage has merged. Keep changes to master behind pull requests.

After every milestone, report overall progress, the current stage and its acceptance/PR status, and the remaining stages.
