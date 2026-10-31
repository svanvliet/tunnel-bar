<script lang="ts">
  import type { AccessRuleRequest } from "./types";
  let { rule = $bindable({ kind: "tenant" }), disabled = false, anonymous = true }: {
    rule?: AccessRuleRequest; disabled?: boolean; anonymous?: boolean;
  } = $props();
</script>

<label class="field"><span>Who can connect</span><select bind:value={rule.kind} {disabled}>
  <option value="tenant">My Entra tenant</option><option value="org">GitHub organization</option><option value="repo">Repository</option>
  {#if anonymous}<option value="anonymous">Anyone (anonymous)</option>{/if}
</select></label>
{#if rule.kind === "org" || rule.kind === "repo"}
  <label class="field"><span>{rule.kind === "org" ? "GitHub organization" : "Repository"}</span><input bind:value={rule.value} placeholder={rule.kind === "org" ? "example-org" : "owner/repo"} required {disabled} /></label>
  <p class="field-hint">{rule.kind === "org" ? "Organization rules require a GitHub sign-in." : "Repository rules are best-effort; provider and CLI restrictions will be shown explicitly."}</p>
{/if}
<div class="two-fields">
  <label class="field"><span>Permission</span><select bind:value={rule.deny} {disabled}><option value={false}>Allow</option><option value={true}>Deny</option></select></label>
  <label class="field"><span>Rule expiration</span><select bind:value={rule.expiration} {disabled}><option value={undefined}>No expiration</option><option value="1h">1 hour</option><option value="8h">8 hours</option><option value="1d">1 day</option><option value="7d">7 days</option><option value="30d">30 days</option></select></label>
</div>
{#if rule.kind === "anonymous" && !rule.deny}<p class="inline-warning">Anyone with the URL will be able to connect without signing in.</p>{/if}
