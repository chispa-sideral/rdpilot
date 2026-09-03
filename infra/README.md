# CI-only DevTest infrastructure

`infra/main.bicep` creates the Azure DevTest Labs foundation for the final
GitHub Actions RDP E2E job: one lab, its dedicated virtual network and subnet,
a formula, and an NSG with no persistent inbound allow rule. The formula can
allocate a public IP so the hosted runner can reach the VM, but the runner adds
an RDP rule only for its current public `/32`, targets the leased VM's private
address, and removes that rule during cleanup.

Deploy this template from an operator-controlled Azure session. The deployment
outputs map directly to GitHub repository variables:

| Bicep output | GitHub variable |
| --- | --- |
| `devtestLabsId` | `AZURE_DEVTEST_LABS_ID` |
| `devtestFormula` | `RDPILOT_DEVTEST_FORMULA` |
| `devtestWorkloadNsgId` | `RDPILOT_DEVTEST_NSG_ID` |

The workflow authenticates with GitHub OIDC. Configure its tenant, client, and
subscription identifiers as repository variables before enabling the E2E gate.
Its Azure identity and a separately reviewed, explicitly scoped external role
are prerequisites: the role must cover the DevTest lease lifecycle, mutation of
the exact lease-owned NSG rule, and deletion/observation of the exact owned
backing resources. This template intentionally creates no role definition or
role assignment. No credentials, connection files, or secrets belong in this
directory.

The DevTest runner also requires `az`, `curl`, Python 3, and the checked-out
repository. It has a 45-minute lease, a 15-minute provisioning limit, a
20-minute E2E command limit, and a 10-minute bounded cleanup attempt. The
workflow has a 55-minute job bound, reserving a further recovery margin for its
`always()` cleanup step. The runner stores only a non-secret lease manifest in
the GitHub runner temporary directory and deletes private connection material
even if Azure cleanup fails. Live deployment and repository-variable/secret
setup are deliberately external to this repository change.
