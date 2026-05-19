# Licensing

nitpik is free for personal, educational, and open-source use. Commercial use requires a subscription.

---

## Free Tier

No license key is needed for:

- **Personal projects** — your own code, side projects, learning
- **Open-source repositories** — any repo with an OSI-approved license
- **Educational use** — classroom, coursework, research

Just install and go. nitpik works at full functionality without a license key.

## Commercial Subscription

For commercial use (proprietary codebases, company projects), subscribe at [nitpik.dev](https://nitpik.dev).

**Monthly or yearly. One flat fee, any team size, unlimited usage.** No per-seat charges, no usage caps. Cancel anytime via the self-serve customer portal.

> **Verify compatibility first:** nitpik's LLM provider integrations rely on a third-party open-source library, and provider support may change due to upstream updates outside of nitpik's control. Before subscribing, please verify that your chosen provider and model work correctly using the free unlicensed version. No license key is required — just install and run a review with your own provider API key. See [LLM Providers](03-Providers) for the full list of supported providers.

## Activating a License

After subscribing, sign in to [nitpik.dev/account](https://nitpik.dev/account) and create an API key. Each key looks like `nkp_live_…` — it's shown once on creation, so copy it somewhere safe.

Store the key in your global config:

```bash
nitpik license activate nkp_live_XXXXXXXXXXXXXXXXXXXXXXXXXX
nitpik license status   # verify activation
```

The key is saved to `~/.config/nitpik/config.toml`.

### In CI

Set the `NITPIK_LICENSE_KEY` environment variable instead:

```bash
export NITPIK_LICENSE_KEY=nkp_live_XXXXXXXXXXXXXXXXXXXXXXXXXX
```

In GitHub Actions:

```yaml
env:
  NITPIK_LICENSE_KEY: ${{ secrets.NITPIK_LICENSE_KEY }}
```

> **Security:** Always store the key as a CI secret — never hardcode it in pipeline files. You can revoke and re-issue keys at [nitpik.dev/account](https://nitpik.dev/account) at any time.

## Managing Your License

```bash
nitpik license activate <KEY>   # store the key in the global config
nitpik license status           # show plan, entitlement type, expiry
nitpik license refresh          # force a fresh entitlement fetch
nitpik license deactivate       # remove the key and the cached entitlement
```

Subscription billing — invoices, plan changes (monthly ↔ yearly), payment method updates, VAT IDs, and cancellation — is handled through the Stripe-hosted customer portal accessible from [nitpik.dev/account](https://nitpik.dev/account).

## How Verification Works

On each review, nitpik exchanges your API key with `nitpik.dev` for a short-lived signed entitlement (an Ed25519 JWT, valid for 7 days). The entitlement is cached at `~/.config/nitpik/entitlement.json` and re-fetched only after it expires.

- **Per-run latency:** one HTTPS round-trip to `nitpik.dev` per fresh CI runner, then no network calls until the cached entitlement expires.
- **Subscription lapses:** the next entitlement fetch fails once your subscription becomes inactive, and the CLI downgrades to free-tier behavior (your existing cached entitlement keeps working until its 7-day window ends).
- **Cryptographic verification:** the entitlement JWT is verified offline against a public key bundled in the nitpik binary. nitpik.dev can stop issuing entitlements but it cannot forge them.

### Air-gapped or restricted-network CI

If your CI runners cannot reach `nitpik.dev`, download an **offline token** from [nitpik.dev/account](https://nitpik.dev/account) (30- or 60-day validity) and supply it via:

```bash
export NITPIK_OFFLINE_TOKEN=<paste-the-jwt-here>
```

When set, `NITPIK_OFFLINE_TOKEN` short-circuits the entire fetch path — the CLI verifies the token's signature and expiry locally and uses it as the proof of entitlement. Offline tokens cannot be revoked before they expire, so pick the shortest window your workflow allows and rotate them on the dashboard before expiry.

## Related Pages

- [Terms of Service](https://nitpik.dev/terms)
- [Privacy Policy](https://nitpik.dev/privacy)
- [Installation](01-Installation) — getting started
- [Configuration](16-Configuration) — license key in config and env vars
- [CI Integration](17-CI-Integration) — setting up the key in pipelines
