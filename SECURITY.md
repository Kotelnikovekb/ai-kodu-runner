# Security policy

## Reporting a vulnerability

Не публикуйте сведения об уязвимости в открытом issue. Отправьте приватное
сообщение владельцу репозитория или используйте GitHub Private Vulnerability
Reporting, если он включён для репозитория.

В сообщении укажите:

- затронутую версию или Docker image digest;
- шаги воспроизведения;
- потенциальное влияние;
- безопасный способ связаться с вами.

Мы постараемся подтвердить получение сообщения в течение 7 дней и сообщить о
плане исправления после проверки.

## Scope

В scope входят Rust runner, Dockerfiles, GitHub/GitLab CI и опубликованные
`tool-images`. Не отправляйте реальные секреты, API keys или персональные данные.

Архитектурная модель угроз Enterprise Edition, residual risks и обязательный
Definition of Secure Done описаны в [`docs/SECURITY_MODEL.md`](docs/SECURITY_MODEL.md).
Она применяется до реализации Enterprise trust boundaries и пересматривается
при изменениях протокола, egress/credential scope, tenant sharing или
Kata/VMM/shared-filesystem boundary.

Security-sensitive trusted services используют отдельные identities,
least-privilege authorization, bounded inputs/quotas и audit. Долгоживущие
секреты не должны храниться в JobSpec, ConfigMap, environment variables, логах
или diagnostic bundles; production adapters получают их через внешний secret
manager/KMS и используют short-lived credentials, где это возможно.
