# Licensing

nitpik is free for personal, educational, and open-source use. Commercial use requires a license.

---

## Free Tier

No license key is needed for:

- **Personal projects** — your own code, side projects, learning
- **Open-source repositories** — any repo with an OSI-approved license
- **Educational use** — classroom, coursework, research

Just install and go. nitpik works at full functionality without a license key.

## Commercial License

For commercial use (proprietary codebases, company projects), purchase a license at [nitpik.dev](https://nitpik.dev).

**One flat fee, any team size, unlimited usage.** No per-seat charges, no usage caps.

> **Verify compatibility first:** nitpik's LLM provider integrations rely on a third-party open-source library, and provider support may change due to upstream updates outside of nitpik's control. Before purchasing a commercial license, please verify that your chosen provider and model work correctly using the free unlicensed version. No license key is required — just install and run a review with your own API key. See [LLM Providers](03-Providers) for the full list of supported providers.

## Activating a License

Store the key in your global config:

```bash
nitpik license activate <YOUR_LICENSE_KEY>
nitpik license status   # verify activation
```

The key is saved to `~/.config/nitpik/config.toml`.

### In CI

Set the `NITPIK_LICENSE_KEY` environment variable instead:

```bash
export NITPIK_LICENSE_KEY=your-key-here
```

In GitHub Actions:

```yaml
env:
  NITPIK_LICENSE_KEY: ${{ secrets.NITPIK_LICENSE_KEY }}
```

> **Security:** Always store the license key as a CI secret — never hardcode it in pipeline files.

## Managing Your License

```bash
nitpik license activate <KEY>   # store a license key
nitpik license status           # show customer, expiry, and validation status
nitpik license deactivate       # remove the key from global config
```

## How Verification Works

License verification is **fully offline**. nitpik validates the key's Ed25519 signature and checks the expiry date locally — no network calls, no license servers, no phone-home.

## Related Pages

- [Installation](01-Installation) — getting started
- [Configuration](13-Configuration) — license key in config and env vars
- [CI/CD Integration](14-CI-Integration) — setting up the key in pipelines
