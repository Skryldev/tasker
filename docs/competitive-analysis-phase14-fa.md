# گزارش تحلیل رقابتی جامع — فاز ۱۴

## Tasker در برابر ۹ ابزار مدیریت کار و پروژه

- **تاریخ تهیه:** ۱۰ نوامبر ۲۰۲۶
- **مبنای کد:** HEAD شاخه `main` (کامیِت `7897d55` — «feat(guests): add team guest role...»)
- **دامنه:** مقایسه Tasker با ClickUp، Things 3، Jira، Plane، Vikunja، Planka، Taiga، OpenProject و Redmine
- **زبان:** فارسی (اصطلاحات فنی استاندارد به انگلیسی حفظ شده‌اند)

> **یادداشت صداقت:** این گزارش بر بازخوانی مستقیم کد Tasker در همین کامیِت استوار است، نه بر گزارش‌های فازهای قبل. هر ادعای مربوط به Tasker با مسیر فایل/ADR/تست ارجاع داده شده و هر ادعای مربوط به رقبا با منبع رسمی. هرجا شاهدی وجود نداشت، صریحاً «نامشخص» یا «وجود ندارد» نوشته شده است.

---

# بخش ۱ — خلاصه اجرایی (Executive Summary)

**Tasker در یک نگاه:** Tasker یک پلتفرم مدیریت کار تیمی self-hosted و MIT-licensed است که با بک‌اند Rust/Axum و فرانت‌اند React/TypeScript ساخته شده و روی PostgreSQL، MySQL/MariaDB و SQLite به‌صورت یکسان اجرا می‌شود (`backend/Cargo.toml`، `docs/database-support.md`، `docker-compose.yml`). قرارداد API آن OpenAPI 3.1 با ۹۰ مسیر و ۱۲۶ عملیات است که به‌صورت contract-first نگهداری می‌شود (`api/openapi.yaml`، `docs/decisions/0007-openapi-contract-first.md`). تمرکز محصول بر چرخه حیات سخت‌گیرانه وظیفه (شش حالت با تأیید Reporter)، امنیت مهندسی‌شده (MFA کامل با TOTP و کدهای بازیابی HMAC، سیاست رمز عبور سخت‌گیرانه، CSRF بدون توکن)، و یک موتور اتوماسیون با الگوی Outbox است (`backend/src/tasks.rs:27-34`، `backend/src/mfa.rs`، `backend/src/rules/`، `docs/rules-engine.md`). پوشش دیدها شامل List، Kanban با swimlane، Timeline به سبک Gantt با کشیدن‌ورها‌کردن، و یک Task Detail تمام‌صفحه است (`frontend/src/pages/TimelinePage.tsx`، `docs/decisions/0015-task-detail-page.md`). تقویم کاربر-محور آن جلالی است (`frontend/src/lib/jalali.ts`، `frontend/e2e/calendar.spec.ts`).

**سه نقطه قوت اصلی در مقایسه با رقبا:**

1. **جریان کاری تأیید Reporter به‌صورت بومی و اجباری:** هیچ‌یک از ۹ رقیب یک چرخه «تحویل → بازبینی → پذیرش/رد با دلیل ممیزی‌شده» را به‌عنوان ستون فقرات محصول ندارند؛ در Jira این فقط در Jira Service Management وجود دارد و در بقیه اصلاً نیست (بخش ۳ و ۴). در Tasker این جریان با نگهبان حالت‌ماشین، ۴۰۹ روی مسابقه وضعیت، و رویدادهای Activity مستند شده است (`backend/src/tasks.rs:1288-1388`، `docs/decisions/0008-acceptance-workflow-notifications.md`، `docs/decisions/0011-task-lifecycle-integrity.md`).
2. **حاکمیت داده واقعی با سه موتور دیتابیس و پروفایل سبک:** ClickUp و Things 3 اصلاً self-hosted نیستند و Jira هم در حال خروج از بازار self-host است (پایان فروش لایسنس Data Center در مارس ۲۰۲۶). Tasker با Docker Compose سه‌پروفایلی، یک SQL پرتابل و تست خودکار روی هر سه موتور، حاکمیت داده می‌دهد بدون هزینه لایسنس (`docker-compose.yml`، `docs/decisions/0004-three-database-engines.md`، `.github/workflows/ci.yml` job «integration-matrix»).
3. **تقویم جلالی به‌عنوان تقویم اصلی محصول:** در بین همه رقبا فقط Redmine فایل ترجمه فارسی در هسته دارد و هیچ‌کدام RTL سراسری ندارند؛ Tasker تقویم شمسی را به‌عنوان تقویم پیش‌فرض انتخاب تاریخ، نمایش تاریخ‌ها با ارقام فارسی، و فیلتر بازه جلالی در Workload ارائه می‌دهد و این با تست e2e تثبیت شده است (`frontend/src/components/ui/date-field.tsx`، `frontend/src/lib/jalali.ts`، `frontend/e2e/calendar.spec.ts:49,104`).

**سه شکاف اصلی:**

1. **فارسی‌سازی ناقص:** اگرچه تقویم جلالی کامل است، **رابط کاربری Tasker انگلیسی و LTR است** — نه RTL سراسری، نه فایل ترجمه، نه فریم‌ورک i18n و نه فونت فارسی وجود دارد (`frontend/src/index.html` با `lang="en"`؛ grep سراسری `frontend/src` هیچ `dir="rtl"`ای پیدا نمی‌کند؛ `tailwind.config.ts:13-24` فقط Inter تعریف کرده که حتی load هم نمی‌شود). Redmine از این نظر (ترجمه فارسی در هسته + RTL جزئی) جلوتر است.
2. **هیچ خروجی اعلان بیرونی وجود ندارد:** اعلان‌ها فقط in-app هستند؛ نه ایمیل، نه push، نه Webhook خروجی، نه یکپارچگی چت. در مقابل، همه رقبای تیمی به‌جز برخی، ایمیل و بیشترشان Webhook دارند (`backend/src/notifications.rs:37-44` فقط ۶ رویداد in-app؛ grep «slack/discord/telegram» در `backend/src` صفر نتیجه؛ `docs/architecture.md` بخش non-goals: ارسال ایمیل هدف فاز ۱ نبوده).
3. **ادامه مسیرهای ناتمام:** یکپارچگی Git فقط زیرساخت است (بدون هیچ endpoint HTTP — `backend/src/integrations/mod.rs:5-7`)، ذخیره‌سازی S3 پیوست‌ها stub است که fail-fast می‌کند (`backend/src/attachments.rs:92-108`)، موتور اتوماسیون هیچ UI ساخت قاعده‌ای ندارد (`frontend/src` هیچ صفحه rules ندارد؛ فقط انواع OpenAPI در `frontend/src/api/schema.d.ts`)، و اعمال وابستگی‌ها advisory است نه blocker (`backend/src/tasks.rs:1911` هیچ بررسی blocker ندارد؛ `backend/src/workspace.rs:310-326` فقط وضعیت را گزارش می‌کند).

**مناسب برای چه تیم‌هایی است:** تیم‌های ۵ تا ۵۰ نفره‌ی ایرانی یا فارسی‌زبان که داده‌شان روی سرور خودشان باید بماند، به چرخه تأیید و شفافیت حسابداری‌شده وظیفه اهمیت می‌دهند، به امنیت حساب کاربری (MFA) حساس‌اند و بودجه لایسنس صفر دارند. **مناسب نیست برای:** سازمان‌هایی که SSO/SAML لازم دارند، تیم‌هایی که اعلان ایمیل/اسلک حیاتی است، تیم‌های Scrum با Sprint/Burndown، تیم‌هایی که ردیابی زمان یا اپ موبایل بومی می‌خواهند، و هرکس UI فارسی کامل امروز لازم دارد.

---

# بخش ۲ — روش‌شناسی (Methodology)

## ۲.۱ منابع سمت Tasker

تمام ادعاهای Tasker از بازخوانی مستقیم کد در کامیِت `7897d55` به‌دست آمده است:

- **کد بک‌اند:** `backend/src/*.rs` (حدود ۲۳٬۰۰۷ خط) شامل `auth.rs`، `security.rs`، `mfa.rs`، `authorization.rs`، `tasks.rs`، `comments.rs`، `activity.rs`، `notifications.rs`، `goals.rs`، `dashboard.rs`، `workspace.rs`، `checklists.rs`، `attachments.rs`، `guests.rs`، `admin.rs`، `projects.rs`، `teams.rs`، `users.rs`، `profile.rs`، `rules/` (۹ فایل)، `integrations/` (۳ فایل)، `http/`.
- **کد فرانت‌اند:** `frontend/src/` (حدود ۲۰٬۷۴۲ خط بدون `schema.d.ts`) شامل صفحات و کامپوننت‌های دیدها.
- **قرارداد API:** `api/openapi.yaml` (۶٬۰۴۸ خط، OpenAPI 3.1.0، ۹۰ مسیر، ۱۲۶ عملیات، ۹۰ schema) و کلاینت تولیدشده `frontend/src/api/schema.d.ts`.
- **ADRها:** `docs/decisions/0001` تا `0019` (ADR-0013 پروژه-حذف به‌طور استثنا در `docs/adr-0013-project-delete.md` قرار دارد، نه در `docs/decisions/`).
- **مستندات عملیاتی:** `docs/architecture.md`، `security.md`، `database-support.md`، `deployment.md`، `configuration.md`، `testing.md`، `performance.md`، `rules-engine.md`.
- **تست‌ها:** `backend/tests/integration.rs` (۱۲٬۱۸۴ خط، ۳۱ تابع `#[tokio::test]` که هر کدام روی تا سه موتور دیتابیس اجرا می‌شوند)، ۱۳۸ تست واحد Rust در `backend/src`، ۱۵ فایل Vitest با حدود ۱۰۳ تست، و ۱۶ فایل Playwright با ۹۹ تست e2e در `frontend/e2e/`.
- **CI:** `.github/workflows/ci.yml` با ۵ job (fmt/clippy/test بک‌اند، typecheck/vitest/build + بررسی drift کلاینت فرانت‌اند، Redocly lint قرارداد، ماتریس یکپارچگی روی postgres:17-alpine و mysql:8.4، و e2e با Playwright).

## ۲.۲ منابع سمت رقبا

- **اولویت اول:** صفحات رسمی قیمت‌گذاری و مستندات هر محصول (clickup.com/pricing، culturedcode.com، atlassian.com و support.atlassian.com، plane.so و developers.plane.so، vikunja.io، planka.app و docs.planka.cloud، taiga.io و docs.taiga.io، openproject.org، redmine.org).
- **اولویت دوم:** مخازن رسمی GitHub برای لایسنس، نسخه جاری، و فایل‌های locale (github.com/makeplane/plane، go-vikunja/vikunja، plankanban/planka، taigaio، opf/openproject، redmine/redmine).
- **اولویت سوم:** نقد‌های معتبر مثل PCMag و TrustRadius فقط برای نقاط ضعف شناخته‌شده، همیشه با لینک.
- **تاریخ دسترسی همه منابع:** ۱۰ نوامبر ۲۰۲۶ (برخی صفحات Atlassian چون scraping زنده را مسدود می‌کند از طریق snapshotهای آرشیو دسامبر ۲۰۲۵/ژانویه ۲۰۲۶ خودِ صفحه‌های رسمی استخراج شده‌اند و در فهرست منابع مشخص شده‌اند).

## ۲.۳ محدودیت‌ها و ادعاهای تأییدنشده

- **مستندات داخلی Tasker گاهی کهنه‌تر از کد است:** README هنوز می‌گوید «Phase 1» و وضعیت شش‌حالتی را پنج‌حالتی توصیف می‌کند؛ `docs/testing.md` شمارش تست‌های فرانت را قدیمی گزارش می‌کند؛ `docs/architecture.md` همه ماژول‌های جدید (mfa، rules، integrations و…) را در نقشه خود ندارد. این گزارش در همه این موارد **کد را ملاک گرفته**، نه مستندات.
- **در جدول مقایسه، سلول‌های ❓ یعنی «در منابع رسمی تأیید نشد»** — نه «ندارد». مثلاً وضعیت RTL در Vikunja/Planka و وجود checklist در Plane از منابع رسمی قابل اثبات نبود.
- **قیمت‌ها** به تاریخ دسترسی وصل‌اند و ممکن است تغییر کنند؛ قیمت ClickUp زنده (نوامبر ۲۰۲۶) و قیمت Jira/Atlassian Guard از snapshot دسامبر ۲۰۲۵ صفحات رسمی است.
- **بازار Cloud رقبا** (uptime، SLA، AI و…) جزء قضاوت این گزارش درباره self-host نیست؛ تمرکز بر ویژگی‌های قابل مقایسه محصولی است.
- چیزهایی که حتی با جست‌وجو تأیید نشدند (مثل شمار دقیق تم‌های رنگی ClickUp یا transations خاص Plane) به‌جای حدس، «نامشخص» ثبت شده‌اند.

---

# بخش ۳ — جدول مقایسه کلی (High-Level Comparison Matrix)

نمادها: ✅ کامل، 🟡 جزئی/مشروط/پولی، ❌ ندارد، ❓ نامشخص (در منابع رسمی تأیید نشد). ارجاع‌های Tasker زیر جدول آمده است.

| ویژگی | Tasker | ClickUp | Things 3 | Jira | Plane | Vikunja | Planka | Taiga | OpenProject | Redmine |
|---|---|---|---|---|---|---|---|---|---|---|
| Self-hosted | ✅ ¹ | ❌ | ❌ | 🟡 ² | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Open source | ✅ (MIT) | ❌ | ❌ | ❌ | 🟡 (AGPL + دو نسخه) | ✅ (AGPL-3.0) | 🟡 (fair-code؛ دیگر MIT نیست) | ✅ (MPL-2.0/AGPL-3.0) | 🟡 (GPLv3 + open-core) | ✅ (GPLv2) |
| رایگان بدون سقف | ✅ | 🟡 | ❌ | 🟡 (تا ۱۰ کاربر) | 🟡 ³ | ✅ | ✅ (Community) | ✅ (خودمیزبان) | 🟡 (Community رایگان، ویژگی‌ها قفل) | ✅ |
| Kanban | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 🟡 ⁴ |
| List View | ✅ | ✅ | ✅ (لیست‌محور) | ✅ | ✅ | ✅ | 🟡 | ✅ | ✅ | ✅ |
| Timeline / Gantt | ✅ | 🟡 ⁵ | ❌ | 🟡 ⁶ | ✅ | ✅ | 🟡 (فقط Pro) | ❌ | ✅ | ✅ |
| Subtasks | ✅ (یک سطح) | ✅ (تا ۷ سطح) | 🟡 (Headings) | ✅ (یک سطح) | ✅ | ✅ | 🟡 (چک‌لیست تخت) | ✅ (Epic→Story→Task) | ✅ | ✅ (بی‌نهایت) |
| Checklists | ✅ | ✅ | ✅ | ❌ | ❓ | 🟡 ⁷ | ✅ | ❌ | ❌ | 🟡 (پلاگین) |
| Dependencies | 🟡 ⁸ | ✅ | ❌ | ✅ | ❓ | ✅ | 🟡 (فقط Pro) | ❌ | ✅ | ✅ |
| Workflow قابل تنظیم | ❌ ⁹ | ✅ | ❌ | ✅ | ✅ | 🟡 | 🟡 | ✅ | ✅ | ✅ |
| Reporter / Approval | ✅ | ❌ | ❌ | 🟡 (فقط JSM) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Comments + Mentions | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | 🟡 (بدون mention) | ✅ | ✅ | ✅ |
| Notifications | 🟡 (فقط in-app) | ✅ (in-app+ایمیل+push) | 🟡 (یادآور شخصی) | ✅ | ✅ | ✅ (ایمیل+in-app) | ✅ (SMTP+سرویس اعلان) | ✅ (ایمیل) | ✅ (in-app+ایمیل) | ✅ (ایمیل) |
| Goals / OKR | ✅ | ✅ (از Core) | ❌ | 🟡 (Atlassian Goals) | 🟡 (فقط Initiatives) | ❌ | ❌ | ❌ | 🟡 (الگوی استفاده) | ❌ |
| Dashboard / KPIs | ✅ | ✅ (از Business) | ❌ | ✅ | 🟡 (بخشی Pro) | ❌ | ❌ (فقط Pro) | ✅ (burndown) | ✅ | 🟡 (پلاگین) |
| Workload / Capacity | ✅ | ✅ (از Business) | ❌ | ✅ (Premium+) | 🟡 | ❌ | ❌ | ❌ | 🟡 (Enterprise Premium) | ❌ |
| Automation | 🟡 ¹⁰ | ✅ (۱۰۰ تا ۱M اکشن/ماه) | ❌ | ✅ | ✅ | 🟡 (n8n خارجی) | ❌ («به‌زودی») | 🟡 (فقط webhook/Zapier) | 🟡 (Enterprise) | ❌ (پلاگین) |
| Git Integration | 🟡 ¹¹ | ✅ | ❌ | ✅ | ✅ (پلن‌های پولی) | ❌ | ❌ | ✅ | ✅ | ✅ (بومی SCM) |
| Webhooks | ❌ ¹² | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ (۵۰+ رویداد) | ✅ | ✅ | 🟡 (پلاگین) |
| MFA / 2FA | ✅ | ✅ | ❌ | 🟡 (با Atlassian Guard) | ✅ (TOTP) | ✅ (TOTP) | ✅ (TOTP از v2.2) | ❌ | ✅ (TOTP+WebAuthn) | 🟡 (پلاگین) |
| SSO / SAML | ❌ | 🟡 (Enterprise) | ❌ | ✅ (با Guard) | ✅ (پلن‌های پولی) | ✅ (OIDC رایگان) | 🟡 (از Community حذف شد → Pro) | 🟡 (پلاگین OIDC) | 🟡 (LDAP رایگان؛ SAML/OIDC پولی) | 🟡 (LDAP بومی؛ SAML پلاگین) |
| Guest / External | ✅ | ✅ (سقف صندلی) | ❌ | 🟡 (مشهدهای JSM) | ✅ (نسبت ۱:۵ در Pro) | ✅ (لینک اشتراک بدون حساب) | 🟡 (فقط Pro) | ✅ (نقش External) | 🟡 (Enterprise) | 🟡 (پروژه عمومی) |
| Persian / RTL | 🟡 ¹³ | ❌ | ❌ | ❌ | ❌ | 🟡 (fa-IR؛ RTL نامشخص) | 🟡 (fa-IR؛ RTL نامشخص) | ❓ | ❌ (RTL پشتیبانی نمی‌شود) | 🟡 (fa در هسته؛ RTL جزئی) |
| Themes | ✅ (۱۶ تم) | 🟡 (دارک/لایت) | 🟡 (دارک مود) | 🟡 (دارک مود) | 🟡 (دارک + تم سفارشی) | 🟡 (لایت/دارک/auto) | 🟡 (۵ تم فقط Pro) | ❓ | 🟡 (تم سفارشی فقط Enterprise) | ✅ (۸۰+ تم اجتماعی) |
| Mobile App | ❌ (فقط وب ریسپانسیو) | ✅ | ✅ (فقط اپل) | ✅ | ✅ | 🟡 (اپ Flutter پایه‌ای) | 🟡 (کامپانون؛ UI موبایل Pro) | ❌ (غیررسمی) | ✅ (اپ رسمی بتا) | ❌ (سوم‌شخص) |
| API | ✅ (OpenAPI 3.1) | ✅ | 🟡 (فقط URL Scheme/AppleScript) | ✅ (REST v3) | ✅ (+SDK/MCP) | ✅ (+CLI) | ✅ (Swagger) | ✅ | ✅ (v3/HATEOAS) | ✅ |
| Attachments | 🟡 (فقط local؛ S3 ناقص) | ✅ (سقف فضای پلن رایگان) | 🟡 (مستند نشده) | ✅ | ✅ | ✅ (local/S3) | ✅ (S3-compatible) | ✅ (سهمیه) | ✅ | ✅ |
| Time Tracking | ❌ | ✅ (از Core) | ❌ | ✅ | 🟡 (پولی) | 🟡 (فقط Pro پولی) | ✅ (کرنومتر در Community) | ❌ | ✅ (+هزینه) | ✅ |

**ارجاع‌های ردیف‌های Tasker:** ¹ `docker-compose.yml` (پروفایل‌های sqlite/postgres/mysql) و `docs/database-support.md`؛ ² —؛ ³ —؛ ⁴ `frontend/src/components/app/KanbanBoard.tsx` با dnd-kit؛ ⁵ —؛ ⁶ —؛ ⁷ `backend/src/checklists.rs`؛ ⁸ `backend/src/workspace.rs:112-136` (جلوگیری از چرخه با BFS) — اما اعمال‌نشده در transition (`backend/src/tasks.rs:1911`)؛ ⁹ شش حالت ثابت در `backend/src/tasks.rs:27-34` — تصمیم معماری، نه نقص تصادفی (ADR-0011)؛ ¹⁰ موتور کامل بک‌اند (`docs/rules-engine.md`) بدون UI (`frontend/src` فاقد صفحه rules)؛ ¹¹ `backend/src/integrations/mod.rs:5-7` — فقط مبنای اعتبارنامه/ذخیره‌سازی، بدون endpoint؛ ¹² فقط جدول `webhook_deliveries` در migration 0016، بدون ارسال/دریافت؛ ¹³ `frontend/src/lib/jalali.ts` + `frontend/src/components/ui/date-field.tsx` + `frontend/e2e/calendar.spec.ts` — اما `frontend/index.html` با `lang="en"` و بدون `dir="rtl"` سراسری.

**ارجاع‌های کلیدی رقبا (خلاصه — فهرست کامل در بخش ۹):** ClickUp ❌ self-host ([CU-onprem])، قیمت/پلن‌ها ([CU-pricing])؛ Jira پایان فروش Data Center ([AT-DC])؛ Planka تغییر لایسنس ([PLK-license]) و حذف SSO از Community ([PLK-rel])؛ OpenProject فهرست ویژگی‌های Enterprise ([OP-enterprise])؛ Redmine locale فارسی ([RM-locales]).

---

# بخش ۴ — تحلیل تفصیلی هر رقیب

## ۴.۱ ClickUp

**معرفی کوتاه.** ClickUp یک SaaS ابری «همه‌چیز-در-یك» برای مدیریت کار، اسناد، چت و اهداف است، میزبان AWS و کاملاً مالکیتی ([CU-onprem], [CU-hosting]).

**نقاط قوت (با منبع):**
- طیف دیدها و ویژگی‌های بسیار گسترده در همه پلن‌ها: List، Board، Table، Calendar، وضعیت‌های سفارشی، وابستگی‌ها و روابط، زیرکارها تا ۷ سطح nesting، چک‌لیست‌ها ([CU-pricing], [CU-subtasks], [CU-deps], [CU-checklists]).
- اتوماسیون صنعتی: از ۱۰۰ اکشن/ماه در Free تا ۱M/ماه در Enterprise؛ GitHub integration در همه پلن‌ها ([CU-pricing], [CU-github]).
- امنیت سازمانی پلکانی: 2FA اپلیکیشنی در همه پلن‌ها؛ SMS/SSO/SAML/SCIM در Business+/Enterprise ([CU-pricing]).
- اپ موبایل iOS/Android، دسکتاپ سه‌سکو، وب و افزونه کروم ([CU-download]).
- API عمومی v2/v3 با Webhook امضادار ([CU-api], [CU-webhooks]).

**نقاط ضعف (با منبع):**
- اصلاً self-hosted نیست؛ ClickUp صراحتاً گفته «گزینه on-premise نداریم» ([CU-onprem]).
- پیچیدگی و هزینه: PCMag (۳.۵/۵): «پیچیدگی‌اش می‌تواند طاقت‌فرسا باشد و هزینه‌ها سریع جمع می‌شوند» ([CU-pcmag]).
- ویژگی‌های کلیدی پولی هستند: Gantt از Core، Timeline/Workload/Dashboard از Business، SSO فقط Enterprise ([CU-pricing]).
- RTL و فارسی پشتیبانی نمی‌شود؛ درخواست RTL همچنان باز است ([CU-langs], [CU-rtl]).

**مدل قیمت‌گذاری.** Free ($0 با ۶۰MB فضا، اعضای نامحدود، ۵ Space)، Core $9/کاربر/ماه سالانه، Business $17 سالانه، Enterprise توافقی، Brain Max $100 ([CU-pricing]).

**مناسب برای.** تیم‌هایی که می‌خواهند همه‌چیز (کار+سند+چت+هدف) در یک SaaS باشد و هزینه و وابستگی ابری را می‌پذیرند.

**مقایسه مستقیم با Tasker.** *کجا Tasker برنده است:* حاکمیت داده و هزینه صفر در مقیاس کامل (Tasker همه ویژگی‌ها را در نسخه MIT می‌دهد؛ `LICENSE`، در برابر Gantt/Dashboard/Workload/SSO قفل‌شده در ClickUp)؛ MFA با کد بازیابی HMAC و رمزنگاری AES-256-GCM رازها در نسخه پایه (`backend/src/mfa.rs`, ADR-0012) در برابر 2FA ساده پایه ClickUp؛ جریان تأیید Reporter اصلاً در ClickUp وجود ندارد. *کجا ClickUp برنده است:* عمق اکوسیستم (اتوماسیون تا ۱M اکشن، ۷ سطح زیرکار، اپ موبایل، ایمیل+push) و داشبوردهای پیشرفته؛ Tasker در همه این‌ها یا ندارد یا نسخه جزئی دارد (بخش ۳).

## ۴.۲ Things 3

**معرفی کوتاه.** Things 3 محصول Cultured Code یک مدیر کار **شخصی** بومی اپل (iPhone/iPad/Mac/Watch/Vision Pro) با خرید یک‌باره است؛ یک ابزار تیمی نیست ([TH-pricing], [TH-support]).

**تأکید ساختاری:** مقایسه Tasker با Things 3 بیشتر «دو رده متفاوت» است تا دو رقیب: Things 3 **اشتراک realtime لیست‌ها را پشتیبانی نمی‌کند** و خودش رسمی گفته «Things در حال حاضر اشتراک real-time وظایف یا لیست‌ها را پشتیبانی نمی‌کند» ([TH-share]).

**نقاط قوت (با منبع):** طراحی و UX تحسین‌شده در اکوسیستم اپل؛ Today/Upcoming با تقویم؛ چک‌لیست داخل وظیفه؛ Areas برای دسته‌بندی؛ Dark Mode؛ همگام‌سازی رایگان Things Cloud ([TH-features]).
**نقاط ضعف (با منبع):** فقط اپل — نه وب، نه اندروید، نه ویندوز ([TH-pcmag])؛ بدون همکاری، بدون کامنت/mention، بدون وابستگی، بدون Gantt، بدون داشبورد، بدون API ابری (فقط URL Scheme و AppleScript محلی) ([TH-features], [TH-urls], [TH-applescript], [MS-things])؛ بدون SSO/MFA سازمانی؛ فاقد زبان فارسی و RTL (۹ زبان رسمی، همه LTR) ([TH-appstore]).

**مدل قیمت‌گذاری.** خرید یک‌باره: iPhone/Watch $9.99، iPad $19.99، Mac $49.99؛ اشتراک ندارد ([TH-pricing], [TH-appstore], [TH-ipad], [TH-mac]).

**مناسب برای.** فردی که مدیریت کار شخصی عمیقاً زیبا در اپل می‌خواهد.

**مقایسه مستقیم با Tasker.** غیرقابل مقایسه در بُعد تیمی: Tasker RBAC پنج‌نقشی، guest grant-based، کامنت/مention/اعلان و Goals سازمانی دارد (`backend/src/authorization.rs:17-25`، `backend/src/guests.rs`)؛ Things 3 هیچ‌کدام را ندارد. برعکس، Things 3 در polish موبایل اپل و سادگی فردی بی‌رقیب است؛ وب‌اپ رسمی Tasker ( responsive تا ۳۷۵px — `frontend/e2e/ux-105.spec.ts:191`) اپ بومی ندارد.

## ۴.۳ Jira

**معرفی کوتاه.** Jira استاندارد دوفاکتوی مدیریت کار مهندسی در بازار سازمانی است؛ Cloud در سه پلن + Enterprise، و نسخه خودمیزبان Data Center که فروش لایسنس جدیدش به ۳۰ مارس ۲۰۲۶ ختم شده و EOL آن ۲۸ مارس ۲۰۲۹ است ([AT-DC], [J-pricing]).

**نقاط قوت (با منبع):** workflow کاملاً قابل تنظیم با validator و property ([J-workflows])؛ داشبوردها و گزارش‌های بلوغ‌یافته (burndown، control chart، CFD، velocity) ([J-dashboards])؛ لینک مسائل با معنای وابستگی در همه پلن‌ها ([J-links])؛ اکوسیستم development بومی (Bitbucket/GitHub/GitLab) ([J-features])؛ Automation با سهمیه‌های پلکانی ([J-pricing])؛ اپ موبایل رسمی ([J-mobile])؛ REST API v3 بلوغ‌یافته ([J-rest]).
**نقاط ضعف (با منبع):** پیچیدگی — PCMag: «در ابتدا می‌تواند طاقت‌فرسا باشد» ([J-pcmag])؛ SSO/2FA سازمانی فقط با خرید جداگانه Atlassian Guard ($4.20-$8.18/کاربر/ماه) ([AT-guard])؛ چک‌لیست native ندارد ([J-support فاقد صفحه checklist])؛ فارسی/RTL پشتیبانی نمی‌شود (فهرست ۲۱ زبان رسمی بدون فارسی) ([AT-langs])؛ خروج تدریجی از self-host ([AT-DC]).

**مدل قیمت‌گذاری.** Cloud: Free تا ۱۰ کاربر (۲GB)، Standard $7.91/کاربر/ماه سالانه، Premium $14.54، Enterprise توافقی؛ Data Center اشتراک سالانه پله‌ای (۵۰۰ کاربر = $51,000/سال) ([J-pricing], [AT-DC]).

**مناسب برای.** سازمان‌های مهندسی بزرگ با فرایندهای رسمی، ردیابی جامع و بودجه.

**مقایسه مستقیم با Tasker.** *Tasker برنده است در:* سادگی کاشی-به-استفاده (شش حالت ثابت با تأیید Reporter در برابر زمین‌بازی workflowهای XML)، MFA بدون افزونه پولی، هزینه صفر، و تست‌شده‌بودن روی SQLite برای استقرارهای کوچک (`backend/tests/integration.rs` روی سه موتور). *Jira برنده است در:* تقریباً همه‌چیز سازمانی — workflow قابل تنظیم، گزارش‌ها، اکوسیستم Git، SSO/SCIM، اپ موبایل، و عمق Marketplace. تفاوت فلسفی: Jira «چارچوبی که باید پیکربندی‌اش کنید» است؛ Tasker «فرایند تثبیت‌شده‌ای که باید آن را بپذیرید».

## ۴.۴ Plane

**معرفی کوتاه.** نزدیک‌ترین رقیب از نظر فلسفه: Plane محصولی مدرن، open source (AGPL-3.0)، self-hostable با Docker، و با تجربه کاربری شبیه Linear/Jira مدرن است؛ ۶۲.۲k ستاره گیت‌هاب و نسخه جاری v1.4.3 ([PLN-gh], [PLN-rel]).

**نقاط قوت (با منبع):** مجموعه دیدها کامل (Board/List/Spreadsheet/Gantt)، Cycles با burndown/velocity، Modules، Pages، Intake ([PLN-features], [PLN-cycles])؛ Docker/K8s رسمی با پنل ادمین «God mode» ([PLN-selfhost])؛ 2FA TOTP ([PLN-2fa])؛ SAML/OIDC در سطوح سازمانی ([PLN-sso])؛ Webhook HMAC و REST API با SDK ([PLN-webhooks], [PLN-api])؛ اپ موبایل iOS/Android ([PLN-features]).
**نقاط ضعف (با منبع):** مدل دو-نسخه‌ای در حال سخت‌تر شدن: نصب‌کننده یک‌خطی جدید «Commercial Edition» است و ویژگی‌هایی مثل integrations، wiki، time tracking و analytics پیشرفته از پلن‌های پولی قفل شده‌اند؛ نگرانی اجتماعی درباره پردیوارسازی Community ثبت شده ([PLN-pricing], [PLN-reddit], [PLN-discussion])؛ فارسی/RTL ندارد و درخواست آن باز است ([PLN-fa-issue]).

**مدل قیمت‌گذاری.** Cloud: Free تا ۱۲ کاربر (بدون integrations/AI/wiki/time tracking)، Pro $8/کاربر/ماه، Business $15، Enterprise توافقی؛ self-host رایگان با کلید لایسنس برای ویژگی‌های پولی ([PLN-pricing]).

**مناسب برای.** تیم‌های محصول مدرن که Linear/Jira ارزان‌تر و self-host می‌خواهند.

**مقایسه مستقیم با Tasker.** این نزدیک‌ترین مقایسه است. *Tasker برنده است در:* سادگی استقرار (یک باینری Rust + یک کانتینر Caddy در برابر استک Django+Redis+Worker+Bird/MQ در Plane — `backend/Dockerfile` در برابر [PLN-selfhost])؛ سه موتور دیتابیس در برابر فقط PostgreSQL ([PLN-gh])؛ تأیید Reporter، Goals/OKR واقعی (برابرِ آن در Plane فقط Initiatives است) و Capacity view که Plane در این بررسی تأیید مستندشده ندارد؛ تست‌های یکپارچگی‌سه‌موتوره. *Plane برنده است در:* بلوغ و اجتماع بزرگ‌تر، دیدهای بیشتر (Spreadsheet)، Cycles/Sprint، اتوماسیون runner، Webhook واقعی، اپ موبایل، SSO، و اپلیکیشن‌های یکپارچه Git (پولی اما موجود). جمع‌بندی: Plane «Jira مدرن و ارزان‌تر» است؛ Tasker «سیستم کار تیمی مینیمال با فرایند تأیید» است.

## ۴.۵ Vikunja

**معرفی کوتاه.** Vikunja یک ابزار مدیریت کار open source سبک با بک‌اند Go و فرانت Vue است؛ AGPL-3.0، نصب به‌صورت یک باینری/کانتینر واحد، نسخه جاری v2.8.0، ~۶k ستاره ([VK-gh], [VK-rel]).

**نقاط قوت (با منبع):** خودمیزبانِ کاملاً رایگان با همه ویژگی‌های هسته و بدون سقف کاربر ([VK-pricing])؛ List/Gantt/Kanban/Table + زیرپروژه‌های سلسله‌مراتبی و وظایف تکرارشونده و relations شامل blocking ([VK-features])؛ Webhook امضاشده با HMAC و محافظ SSRF ([VK-webhooks])؛ OIDC رایگان با همگام‌سازی تیم از claims ([VK-openid])؛ TOTP پیش‌فرض فعال ([VK-security])؛ فایل fa-IR.json در ترجمه‌های فرانت شِپ می‌شود ([VK-i18n])؛ API مستند Swagger/OpenAPI 3.1 + CLI ([VK-api])؛ CalDAV و ایمپورت از Todoist/Trello ([VK-features]).
**نقاط ضعف (با منبع):** time tracking به ویژگی پولی «Vikunja Pro» (بتای خصوصی) منتقل شده — نمونه‌ای از فشار درآمدی روی مدل رایگان ([VK-pro], [VK-gh2521])؛ هیچ Goals/OKR، هیچ Workload/Capacity، هیچ اتوماسیون داخلی (فقط n8n)، هیچ یکپارچگی Git در هسته ندارد ([VK-features])؛ اپ موبایل Flutter «فقط ویژگی‌های خیلی پایه» ([VK-app])؛ RTL در مستندات تأییدنشده (❓).

**مدل قیمت‌گذاری.** خودمیزبان رایگان مطلق (AGPL)؛ Cloud اروپایی Personal €4/ماه، Family €10، Organization €5/کاربر/ماه؛ Pro پولی ([VK-pricing]).

**مناسب برای.** تیم‌ها و افراد کوچک که یک ابزار سبکِ کاملاً-رایگانِ خودمیزبان می‌خواهند.

**مقایسه مستقیم با Tasker.** دو «سادگیِ خودمیزبان» رو در رو. *Tasker برنده است در:* معماری نقش‌ها و امنیت سازمانی (RBAC پنج‌نقشی، MFA با کد بازیابی و رمزنگاری راز، سیاست رمز ۱۲+، guest grants — `backend/src/authorization.rs`، `backend/src/mfa.rs`، `backend/src/security.rs` در برابر مدل تیم ساده Vikunja)، داشبورد KPI و Workload/Capacity (Vikunja هیچ‌کدام را ندارد)، Goals/OKR، تقویم جلالی، و معیارهای مهندسی کیفیت (قرارداد OpenAPI مدیریت‌شده + ماتریس تست سه‌موتوره در CI — `.github/workflows/ci.yml`). *Vikunja برنده است در:* تنوع دیدها (تقویم/جداول ذخیره‌شده)، وظایف تکرارشونده، Webhook خروجی واقعی، ایمیل نوتیفیکیشن، OIDC آماده، CalDAV، ترجمه فارسی شِپ‌شده، و اجتماع بالغ‌تر. این جفت‌ترین رقیب از نظر «سبکی» است.

## ۴.۶ Planka

**معرفی کوتاه.** Planka یک برد Kanban مینیمال self-hosted با realtime (Node.js/Sails + PostgreSQL) است؛ ۱۲.۷k ستاره، نسخه v2.3.1، توسعه‌دهنده یک شرکت آلمانی ([PLK-gh], [PLK-rel]).

**تأکید ساختاری: Kanban-only.** Planka تعمداً فقط Kanban است؛ Timeline، Calendar، داشبورد کاربر، قالب‌ها و حتی لینک کارت‌به‌کارت (وابستگی) همگی به PLANKA Pro منتقل شده‌اند ([PLK-features]).

**نقاط قوت (با منبع):** تجربه Kanban روان با realtime WebSocket، چک‌لیست با پیشرفت، custom fields، برچسب‌های ۴۰+ رنگ، API با Swagger، Webhook با ۵۰+ رویداد، SMTP و سرویس اعلان «۱۰۰+ ارائه‌دهنده»، S3-compatible storage، TOTP 2FA (از v2.2)، ۳۵+ زبان شامل fa-IR، و time tracking/کرنومتر — همه در Community ([PLK-features], [PLK-docs], [PLK-locales]).
**نقاط ضعف (با منبع):** **دیگر open source نیست**: از MIT به «PLANKA Community License / Fair Use v1.1» تغییر یافته — source-available و «fair code»، مجاز برای استفاده داخلی/شخصی اما میزبانی تجاری برای ثالث ممنوع بدون لایسنس پولی ([PLK-license])؛ SSO/OIDC در v2.2.0 از Community **حذف** شد و به Pro رفت (تغییر شکستن‌دهنده) ([PLK-rel])؛ نارضایتی اجتماعی از پردیوارسازی ثبت شده ([PLK-1640])؛ بدون اهداف، بدون workload، بدون اتوماسیون («به‌زودی»)، بدون یکپارچگی Git ([PLK-features]).

**مدل قیمت‌گذاری.** Community رایگان؛ Pro Hosted €8.50/کاربر/ماه (حداقل ۵ کاربر)، Pro Self-Hosted €7.20/کاربر/ماه، Enterprise توافقی ([PLK-pricing]).

**مناسب برای.** تیم‌هایی که فقط یک برد Kanban سریع و زیبا می‌خواهند.

**مقایسه مستقیم با Tasker.** *Tasker برنده است در:* لایسنس (MIT پایدار در برابر fair-code در حال سخت‌شدن)، دامنه (چرخه تأیید، Goals، داشبورد، Capacity، Timeline رایگان در برابر Pro-قفل Planka)، امنیت حساب (MFA کامل Tasker از ابتدا؛ TOTP Planka تازه در v2.2)، و دیتابیس‌های سه‌گانه در برابر PostgreSQL-only. *Planka برنده است در:* سادگی مطلق برای Kanban-only، realtime بلوغ‌یافته، اعلان ایمیل آماده، Webhook خروجی واقعی، ترجمه fa-IR، و time tracking. نکته راهبردی: مسیر Planka (قفل‌کردن تدریجی Community) هشداری برای مدل درآمدی هر پروژه open source است.

## ۴.۷ Taiga

**معرفی کوتاه.** Taiga ابزار کلاسیک open source چابک (Scrum/Kanban) با بک Django و فرانت Angular است؛ از مارس ۲۰۲۴ مالکیت از Kaleidos به Taiga Cloud Services منتقل شده ([TG-gov]).

**تأکید ساختاری: Scrum-first.** Backlog، Sprint، taskboard، تخمین (Seed)، burndown پروژه و اسپرینت در هسته ([TG-site]).

**نقاط قوت (با منبع):** Scrum/Kanban کامل با swimlane و WIP limit ([TG-site])؛ وضعیت‌های سفارشی و custom fields ([TG-custom])؛ یکپارچگی‌های رسمی GitHub/GitLab/Bitbucket/Gogs و Slack ([TG-integrations])؛ Webhook مستند ([TG-webhooks])؛ API کامل REST ([TG-api])؛ قیمت‌گذاری SaaS با کاربر نامحدود ([TG-pricing])؛ ماژول Wiki و Issues ([TG-custom]).
**نقاط ضعف (با منبع):** هیچ Gantt ندارد — «Timeline» آن صرفاً فید فعالیت است ([TG-timeline])؛ وابستگی first-class ندارد و همچنان درخواست باز است ([TG-deps-issue])؛ چک‌لیست مستند ندارد؛ time tracking ندارد؛ MFA native ندارد (تأیید فروم رسمی) ([TG-mfa])؛ بدون اپ موبایل رسمی ([TG-mobile])؛ **ریسک تداوم توسعه**: Taiga Next لغو شد، Kaleidos رفت، تیم سابق Tenzu را ساخت — نشانه‌های کندی نقشه راه ([TG-gov], [TG-tenzu])؛ وضعیت ترجمه فارسی نامشخص (❓) ([TG-langs]).

**مدل قیمت‌گذاری.** SaaS: Free (۱ پروژه عمومی + ۱ خصوصی، ۱۰MB)، Enthusiast €5، Basic €20، Premium €60 — همه با کاربر نامحدود؛ Private Cloud/On-Premise توافقی ([TG-pricing]).

**مناسب برای.** تیم‌های Scrum کلاسیک با بودجه کم.

**مقایسه مستقیم با Tasker.** *Tasker برنده است در:* Gantt/Timeline (Taiga ندارد)، وابستگی‌ها با جلوگیری از چرخه (Taiga ندارد)، MFA کامل (Taiga ندارد)، Goals/KPI داشبورد، اعلان in-app، استقرار مینیمال (یک کانتینر در برابر استک ۶-سرویسی Django+RabbitMQ+nginx — `docker-compose.yml` در برابر [TG-docker])، و لایسنس MIT بدون التزام copyleft. *Taiga برنده است در:* پشتیبانی Scrum واقعی (Sprint/burndown/تخمین)، ترجمه‌های چندزبانه، یکپارچگی‌های Git آماده، Webhook، اجتماع تاریخی بزرگ‌تر، و SaaS آماده. Tasker امروز هیچ معادل Sprint ندارد — برای تیم Scrum کلاسیک انتخاب نیست.

## ۴.۸ OpenProject

**معرفی کوتاه.** OpenProject بزرگ‌ترین پلتفرم مدیریت پروژه open source اروپایی (آلمانی، GPLv3) با مدل open-core صریح است؛ نسخه جاری 17.x، ~۱۷.۲k ستاره ([OP-gh], [OP-rel]).

**تأکید ساختاری: Gantt و Enterprise.** ماژول Gantt کامل (کشیدن‌ورها‌کردن زمان‌بندی، milestone، لینک پیش‌نیاز/پیرو با time-shift خودکار، روزهای کاری) رایگان است ([OP-gantt]).

**نقاط قوت (با منعه):** Gantt رده‌بالا، hierarchies و relations غنی (relates/predecessor/blocks/includes/requires) ([OP-relations])؛ Kanban رایگان (از 12.1/17.3 کامل شد) ([OP-boards])؛ داشبوردهای شخصی/پروژه رایگان ([OP-widgets])؛ 2FA رایگان از 12.4 (TOTP+WebAuthn) ([OP-2fa])؛ LDAP رایگان؛ Webhook و API v3 رایگان؛ یکپارچگی GitHub رایگان؛ Time & cost tracking با بودجه رایگان ([OP-pricing], [OP-time])؛ ۴۰+ زبان ([OP-translations])؛ اپ موبایل رسمی ([OP-mobile]).
**نقاط ضعف (با منبع):** مدل open-core تهاجمی: SSO (SAML/OIDC)، اشتراک work package با مهمان‌ها، تم سفارشی، Team Planner، resource management، portfolio، internal comments — همگی Enterprise پولی ([OP-enterprise])؛ نصب توصیه‌شده بسته‌ای DEB/RPM روی لینوکس و مرور مخزن Git در Docker در دسترس نیست ([OP-docker])؛ RTL پشتیبانی نمی‌شود («RTL languages are not specifically supported») ([OP-rtl])؛ چک‌لیست native ندارد ([OP-feature-request])؛ نارضایتی اجتماعی از شِپ‌شدن کد قفل‌شده داخل مخزن GPLv3 ([OP-reddit]).

**مدل قیمت‌گذاری.** Cloud: Basic €5.95 (حداقل ۲۵ کاربر)، Professional €10.95، Premium €15.95 (حداقل ۱۰۰)، Corporate توافقی؛ Enterprise on-prem پله‌ای مشابه ([OP-pricing]).

**مناسب برای.** سازمان‌ها و PMOهایی با نیاز به Gantt/cost/Portfolio که open source آلمانی می‌خواهند.

**مقایسه مستقیم با Tasker.** مقایسه‌ای نابرابر از نظر اندازه. *Tasker برنده است در:* سادگی استقرار سبک (Docker Compose سه‌پروفایلی + SQLite ممکن — `docker-compose.yml` در برابر پیشنهاد packaged-install OpenProject)، مهمان grant-based رایگان (در OpenProject Enterprise Professional!)، داشبورد KPI و Capacity بدون قفل، و تقویم جلالی. *OpenProject برنده است در:* تقریباً هر بُعد بلوغ سازمانی — Gantt حرفه‌ای با auto-rescheduling، SSO، portfolio/resource management، wiki، هزینه/زمان، ۴۰+ زبان، اپ موبایل، اجتماع بزرگ. برای سازمان متوسط به بالا، OpenProject انتخاب پیش‌فرض open-source است؛ Tasker جایگاه «تیم کوچک ساده‌خواه» را نگه می‌دارد.

## ۴.۹ Redmine

**معرفی کوتاه.** قدیمی‌ترین بازیگر فهرست (از ۲۰۰۶)، Ruby on Rails، GPLv2، **کاملاً رایگان بدون هیچ تیتر پولی**؛ نسخه جاری 6.0/6.1/7.0 (نوامبر ۲۰۲۶) ([RM-site], [RM-gh]).

**تأکید ساختاری: رایگان مطلق و پلاگین‌محور.** همه چیز در هسته یا از طریق اکوسیستم بزرگ پلاگین (فهرست رسمی در redmine.org/plugins) ([RM-plugins]).

**نقاط قوت (با منبع):** زیرtask بی‌نهایت عمق، workflow/ tracker/field کاملاً قابل تنظیم، ردیابی زمان built-in، Gantt built-in، روابط کامل (blocks/precedes و…)، مرور مخزن SCM بومی (Git/SVN/Mercurial/…)، REST API بالغ، LDAP بومی، **فایل locale فارسی در هسته** و `rtl.css` جزئی، ~۸۰+ تم اجتماعی، Wiki و فروم هر پروژه ([RM-site], [RM-issues], [RM-repos], [RM-api], [RM-ldap], [RM-locales], [RM-rtl], [RM-themes]).
**نقاط ضعف (با منبع):** Kanban فقط با پلاگین (RedmineUP Agile؛ PRO پولی) ([RM-agile])؛ 2FA/SAML/Webhook/Dashboard همگی پلاگین‌ثالث‌اند ([RM-plugins])؛ بدون اپ رسمی؛ **نصب دستی فقط** — هیچ Docker رسمی در مستندات ([RM-install])؛ UI قدیمی — TrustRadius: «با کنار گذاشتن UI قدیمی، Redmine ابزار عالی رایگان است» ([RM-trustradius])؛ بدون اعلان in-app مدرن/داشبورد KPI در هسته.

**مدل قیمت‌گذاری.** رایگان مطلق؛ فقط پلاگین‌های تجاری ثالث پولی‌اند.

**مناسب برای.** سازمان‌های فنی صبور که رایگانِ مطلق و انعطاف بی‌پایان می‌خواهند و هزینه نگهداری/پلاگین را می‌پذیرند.

**مقایسه مستقیم با Tasker.** *Tasker برنده است در:* تجربه به‌روز (UI الهام Linear در برابر UI دهه-۲۰۰۰)، نصب Docker یک‌فرمانی، امنیت مدرن پیش‌فرض (MFA/CSRF/سیاست رمز — در Redmine پلاگین‌خواه)، اعلان in-app، داشبورد KPI، Views مدرن با dnd-kit، و contract-first بودن API. *Redmine برنده است در:* بلوغ ۲۰-ساله، ترجمه فارسی واقعی در هسته + RTL جزئی (Tasker فقط تقویم جلالی دارد)، time tracking، عمق بی‌نهایت زیرtask، workflow قابل تنظیم، مرور مستقیم مخازن Git، و هزینه صفر ابدی بدون ریسک تغییر مدل درآمدی. Redmine «چاقوی سوئیسی کهنه اما بی‌رقیب» است؛ Tasker «تجربه مدرن برای تیم کوچک».

---

# بخش ۵ — مقایسه موضوعی (Thematic Comparison)

## ۵.۱ خودمیزبانی و حاکمیت داده

طیف واقعی این‌گونه است: **SaaS خالص** — ClickUp (بدون گزینه on-premise [CU-onprem]) و Things 3 (بدون سرور اصلاً؛ همگام‌سازی فقط از طریق Things Cloud [TH-support]). **در حال خروج از self-host** — Jira: فروش لایسنس Data Center به مارس ۲۰۲۶ محدود و EOL در ۲۰۲۹ [AT-DC]. **Open source کامل** — Vikunja (AGPL، همه ویژگی‌های هسته رایگان [VK-pricing])، Taiga (MPL/AGPL [TG-gov])، Redmine (GPLv2 بدون پول [RM-site]). **Open core / source-available** — OpenProject (GPLv3 + افزونه Enterprise روی همان کد [OP-enterprise])، Plane (AGPL اما integrations/wiki/time-tracking قفل [PLN-pricing] و نگرانی اجتماعی [PLN-reddit])، Planka (fair-code؛ دیگر MIT نیست و SSO از Community حذف شد [PLK-license], [PLK-rel]).

**جایگاه Tasker:** کد کامل MIT و self-host واقعی است — مثل گروه Redmine/Vikunja/Taiga و برخلاف سه‌گانه open-core. تفاوت متمایز آن: **قابلیت حمل دیتابیس** — یک SQL پرتابل روی PostgreSQL/MySQL-MariaDB/SQLite با تست CI سه‌موتوره (`docs/decisions/0003-portable-sqlx-any.md`، `docs/decisions/0004-three-database-engines.md`، `.github/workflows/ci.yml` job «integration-matrix») — هیچ رقیبی این را ندارد (همگی فقط PostgreSQL یا + MySQL). نقطه ضعف صادقانه: محدودیت SQLite (نویسنده-منفرد با ۵۰۳ Retry-After) و عدم سقف‌افزایی مستند در `docs/database-support.md` ذکر شده، و rate limiter پردازه-محل است (`docs/security.md` «Known limitations») — پس برای استقرارهای چندریپلیکا هنوز طراحی نشده.

## ۵.۲ جریان کاری و مدیریت وظیفه

چهار مدل در بازار وجود دارد: **جریان ثابت سخت‌گیرانه با تأیید** (Tasker: شش حالت `backlog→todo→in_progress→resolved→in_review→closed`، فقط transition مسیر محور، Reject با دلیل اجباریِ ممیزی‌شده، ممنوعیت self-review، ۴۰۹ روی race — `backend/src/tasks.rs:27-34,1288-1388`؛ `docs/decisions/0011-task-lifecycle-integrity.md`)؛ **قابل تنظیم سازمانی** (Jira: workflowهای دلخواه با validator [J-workflows]؛ Redmine/OpenProject مشابه [RM-site], [OP-relations])؛ **انعطاف‌پذیر SaaS** (ClickUp: وضعیت سفارشی در همه پلن‌ها [CU-pricing])؛ **مدرن نیمه-ثابت** (Plane: وضعیت‌های سفارشی هر پروژه [PLN-features]؛ Taiga: افزودن/رنگ/ترتیب وضعیت [TG-custom])؛ و **ثابت‌های ساده** (Planka: ستون‌های آزاد اما بدون مفهوم status [PLK-features]؛ Vikunja: فقط done [VK-features]؛ Things 3: بدون وضعیت اصلاً [TH-features]).

**کدام مدل برای چه تیمی؟** Tasker هنگامی درست است که فرایند شما واقعاً «کار → تحویل → بازبینی → پذیرش» باشد و می‌خواهید نرم‌افزار انضباط را اجرا کند (نگهبان سرور حتی از Owner هم عبور نمی‌کند — «no role override bypasses the matrix»، `backend/src/tasks.rs:1279-1286`). اگر فرایند شما ماتریس‌های چندشاخه‌ای دارد، Jira/Redmine/OpenProject لازم است؛ اگر کل فرایند شما «ستون‌ها را جابه‌جا کن» است، Planka/Vikunja سبک‌ترند. نکته صادقانه: انعطاف‌ناپذیری Tasker هم ویژگی است هم محدودیت — هیچ مسیر مهاجرت وضعیت سفارشی در OpenAPI نیست (`api/openapi.yaml`).

## ۵.۳ همکاری و ارتباطات

Tasker در هسته همکاری قوی است: کامنت تِرِدد تا عمق ۲ با @mention محدود به اعضای فعال تیم (`backend/src/comments.rs:306-311,121-151`)، Activity تراکنشی ۱۶-رویدادی در همان transaction جهش داده (`backend/src/activity.rs:50-82`)، اعلان in-app با badge و شش ترجیح خاموش‌کردنی (`backend/src/notifications.rs:37-44`)، و مهمان‌های grant-based که با 404 نامرئی می‌شوند (`backend/src/guests.rs:4-6`).

اما در کانال‌های بیرونی، Tasker **آخر جدول است**: همه رقبای تیمی به‌جز Planka-Community-قدیم، ایمیل دارند؛ همه به‌جز Taiga/Vikunja-هسته/Redmine-هسته Webhook خروجی دارند (بخش ۳). Tasker نه ایمیل می‌فرستد، نه push، نه Webhook، نه اسلک — و ارسال ایمیل در فاز ۱ صراحتاً non-goal بوده (`docs/architecture.md`)؛ این یعنی همکاری Tasker «همیشه-داخل-اپ» است و برای تیم‌های توزیع‌شده‌ای که صندوق ایمیلشان مرکز جهان است، گره‌گاه تجربه است. در مقابل، Part-3 روایت مهمان Tasker (دو لایه: project viewer قدیمی + team guest جدید با ۵ مجوز صریح — `backend/src/guests.rs:34-40`) از مهمان‌پردازی OpenProject (فقط Enterprise!) و Planka (فقط Pro) کامل‌تر و از ClickUp (سقف صندلی) ساده‌تر است.

## ۵.۴ دیدها و گزارش‌دهی

Tasker چهار دید دارد و هر چهار با کیفیت مهندسی‌شده: List گروه‌بندی-وضعیتی با مرتب‌سازی دستی (`frontend/src/components/app/TaskTable.tsx:11-18`)، Kanban با swimlane سه‌بعدی (assignee/priority/label) و dnd-kit + Undo (`KanbanBoard.tsx:196,231-275`؛ `ProjectPage.tsx:406-456`)، Timeline با zoom سه‌سطحی، گروه‌بندی، drag-reschedule با تأیید، فلش‌های وابستگی و الماس deadline (`frontend/src/pages/TimelinePage.tsx:29-46`)، و Task Page تمام‌صفحه با rail متادیتا و جداسازی Activity از Comments (ADR-0015/0019؛ `frontend/src/pages/TaskPage.tsx`). داشبورد ۸ KPI سرورمحور دارد: throughput، cycle time، lead time، WIP، overdue، deadline risk، review load و capacity utilization (`backend/src/dashboard.rs:41-53`) + Workload و Capacity با سقف شخصی (`backend/src/workspace.rs:431,1106`).

مقایسه: ClickUp/OpenProject/Jira در **تعداد** دیدها جلوترند (تقویم، اسپردشیت، Portfolio)؛ Planka/Vikunja در **سادگی**. در **گزارش‌های مدیریتی عمیق** (burndown، velocity، CFD) Tasker ندارد و به Taiga (Scrum) و Jira و OpenProject می‌بازد. در **برابری دید Timeline رایگان**، Tasker از ClickUp (Business-قفل) و Planka (Pro-قفل) جلوتر و هم‌تراز OpenProject/Vikunja است. دو حفره صادقانه: (۱) Kanban Tasker **جابه‌جایی داخل ستون ندارد** (فقط تغییر status — `KanbanBoard.tsx:231-275`) و (۲) هیچ نمای تقویم ماهانه و هیچ burndown وجود ندارد.

## ۵.۵ امنیت و کنترل دسترسی

Tasker در امنیتِ آنچه *دارد* رقبا را پشت سر می‌گذارد و در آنچه *ندارد* صادقانه عقب است:

- **دارد:** سشن کوکی با توکن opaque هش‌شده و revoke؛ CSRF توکن‌less دولایه (SameSite=Lax + Origin/Sec-Fetch-Site)؛ rate limit ورود؛ Argon2id؛ سیاست رمز ۱۲+ با blocklist و قاعده هویت (`backend/src/security.rs`، `backend/src/http/mod.rs:178-220`، ADR-0005/0016)؛ **MFA کامل**: TOTP با window=0 و ضدپخش CAS، راز رمزنگاری‌شده AES-256-GCM، توکن معلق `mfp_` تک‌مصرف ۱۰ دقیقه‌ای، کدهای بازیابی HMAC-SHA-256 با pepper (ADR-0012) (`backend/src/mfa.rs`)؛ RBAC پنج‌نقشی متمرکز ۱۳-capability با Guest deny-by-default (`backend/src/authorization.rs:93-131`)؛ ایزوله‌سازی بین‌تیمی با 404 (`docs/decisions/0017-cross-team-isolation.md`).
- **ندارد:** SSO/SAML/OIDC (هیچ‌کدام از رقبا هم رایگان ندارند به‌جز Vikunja [VK-openid]؛ OpenProject و Planka آن را پولی کردند [OP-enterprise], [PLK-rel])، WebAuthn و ایمیل بازیابی رمز (`docs/security.md` Known limitations)، audit log تغییرات عضویت (همان‌جا)، rate limiter مشترک چندریپلیکا.

تراز نهایی: در MFA فقط OpenProject (با WebAuthn رایگان!) هم‌تراز و بقیه یا پولی‌اند (Jira/Guard، ClickUp Enterprise برای اجبار) یا ندارند (Taiga، Things 3). در guest، Tasker رده‌بالا است. در SSO، Tasker تنها بازیگر فهرست بدون هر مسیر SSO است — شکاف واقعی برای هر فروش سازمانی.

## ۵.۶ بومی‌سازی و بازار ایران

**این مهم‌ترین بخش تمایز است — با دقت کامل روایت شود.**

آنچه Tasker *دارد* (اثبات‌شده با تست): تقویم جلالی به‌عنوان تقویم کاربر-محور — انتخاب تاریخ با پیکر فارسی/RTL (کتابخانه Doran با locale faIR)، تبدیل مستقل میلادی↔جلالی بدون وابستگی، نمایش با ارقام فارسی (۱۴۰۵/۰۸/۰۹)، فیلتر بازه جلالی در Workload، و انکرهای Nowruz در تست واحد و e2e که «تقویم فارسی، تقویم کاربر است و API میلادی می‌گیرد» را تثبیت می‌کنند (`frontend/src/lib/jalali.ts`، `frontend/src/lib/doran.ts:32`، `frontend/src/components/ui/date-field.tsx`، `frontend/src/lib/__tests__/jalali.test.ts`، `frontend/e2e/calendar.spec.ts:49,104`). timezone سیاست‌گذاری‌شده `Asia/Tehran` (`date-field.tsx:3-26`).

آنچه Tasker *ندارد* (صداقت کامل): **RTL سراسری** (هیچ `dir="rtl"` در پوسته — فقط داخل پیکر تاریخ)، **زبان فارسی UI** (همه رشته‌ها انگلیسی هاردکد؛ بدون i18n framework)، **فونت فارسی** (فقط Inter تعریف‌شده که load هم نمی‌شود — `tailwind.config.ts:13-24`).

وضعیت رقبا در فارسی: ClickUp ❌ (۷ زبان اروپایی + درخواست RTL باز [CU-langs], [CU-rtl])؛ Things 3 ❌ (۹ زبان، همه LTR [TH-appstore])؛ Jira ❌ (۲۱ زبان بدون فارسی [AT-langs])؛ Plane ❌ (درخواست fa+RTL باز [PLN-fa-issue])؛ Vikunja 🟡 (fa-IR.json شِپ می‌شود؛ RTL نامشخص [VK-i18n])؛ Planka 🟡 (fa-IR در ۳۵ locale؛ RTL نامشخص [PLK-locales])؛ Taiga ❓ ([TG-langs])؛ OpenProject ❌ برای RTL («not specifically supported» [OP-rtl])؛ **Redmine 🟡 پیشتاز: fa.yml در هسته + rtl.css جزئی + تم‌های فارسی اجتماعی** ([RM-locales], [RM-rtl], [RM-themes]).

**نتیجه تماتیک:** مزیت منحصربه‌فرد فعلی Tasker «تقویم جلالی اول-کلاس» است — چیزی که هیچ رقیبی حتی Redmine هم به این کیفیت (انتخابگر + نمایش + فیلتر + تست) ارائه نمی‌دهد. اما ادعای «بومی‌سازی فارسی کامل» امروز صادق نیست: برای تبدیل این مزیت به «پلتفرم فارسی واقعی»، سه کار باقی است — RTL سراسری، فایل‌های ترجمه با فریم‌ورک i18n، و فونت فارسی (مثلاً Vazirmatn). این دقیقاً همان فاصله‌ای است که Redmine (ترجمه) و هیچ‌کس (RTL باکیفیت) پُر نکرده‌اند — بازار هدف طبیعی Tasker.

---

# بخش ۶ — ماتریس تصمیم (Decision Matrix)

امتیاز ۱ (ضعیف) تا ۵ (عالی)، از منظر همان سناریو. Things 3 در جدول نیست چون ابزار شخصی غیرتیمی است و در هیچ سناریوی تیمی زیر معنادار نیست.

| سناریو | Tasker | ClickUp | Jira | Plane | Vikunja | Planka | Taiga | OpenProject | Redmine |
|---|---|---|---|---|---|---|---|---|---|
| استارتاپ ۵ نفره، بودجه صفر | **۵** | ۴ | ۳ | ۴ | ۵ | ۴ | ۴ | ۳ | ۳ |
| تیم ۲۰ نفره، نیاز به self-host | **۵** | ۱ | ۲ | ۴ | ۵ | ۴ | ۴ | ۴ | ۴ |
| تیم ایرانی، نیاز به فارسی | **۴** | ۱ | ۱ | ۱ | ۳ | ۲ | ۲ | ۱ | ۳ |
| تیم مهندسی، نیاز به Git | ۲ | ۴ | **۵** | ۴ | ۲ | ۱ | ۴ | ۴ | **۵** |
| سازمان بزرگ، نیاز به SSO | ۱ | ۴ | **۵** | ۴ | ۳ | ۲ | ۲ | ۴ | ۳ |
| تیم Scrum | ۲ | ۴ | **۵** | ۴ | ۳ | ۱ | **۵** | ۴ | ۳ |
| تیم ساده، فقط Kanban | ۴ | ۴ | ۲ | ۴ | ۴ | **۵** | ۳ | ۳ | ۲ |

**دلیل امتیازها:**

1. **استارتاپ ۵ نفره، بودجه صفر.** Tasker ۵: همه ویژگی‌ها رایگان MIT، نصب SQLite یک‌فرمانی، بدون سقف (`docker-compose.yml` پروفایل sqlite؛ `LICENSE`). Vikunja ۵: کاملاً رایگان بالغ‌تر با دیدهای بیشتر. ClickUp ۴: Free واقعاً کارآمد اما ۶۰MB فضا و قفل Gantt/Workload ([CU-pricing]). Planka ۴: عالی برای کانبان اما بدون چرخه تأیید. Jira ۳: Free خوب است تا ۱۰ کاربر ولی برای غیر-مهندس‌ها سنگین. Taiga ۴: SaaS رایگان فقط ۲ پروژه. OpenProject ۳: min ۲۵ کاربر در Cloud و پیچیدگی بالا ([OP-pricing]). Redmine ۳: رایگان اما هزینه راه‌اندازی/نگهداری بالا.
2. **تیم ۲۰ نفره، self-host.** Tasker ۵: self-host بدون لایسنس، سه دیتابیس، MFA و RBAC آماده. Vikunja ۵: مشابه و بالغ‌تر. ClickUp ۱: اصلاً نصب نمی‌شود ([CU-onprem]). Jira ۲: DC دیگر فروخته نمی‌شود؛ مسیر جدیدی نیست ([AT-DC]). Plane ۴: نصب آسان اما ویژگی‌ها با کلید لایسنس ([PLN-pricing]). Planka ۴: سبک ولی fair-code و بدون SSO رایگان. Taiga ۴: نصب Docker رسمی چندسرویسه. OpenProject ۴: قوی ولی سنگین و قفل‌های Enterprise. Redmine ۴: نصب دستی و نگهداری بالا ([RM-install]).
3. **تیم ایرانی، فارسی.** Tasker ۴: تقویم جلالی کامل و تست‌شده، اما نه RTL سراسری نه UI فارسی — لذا ۴ نه ۵ (بخش ۵.۶). Vikunja ۳: ترجمه fa دارد ([VK-i18n]). Redmine ۳: fa هسته + RTL جزئی ولی UI قدیمی ([RM-locales], [RM-rtl]). Planka ۲: fa-IR ولی RTL نامشخص و SSO حذف‌شده. Taiga ۲: نامشخص؛ بقیه ۱.
4. **تیم مهندسی، Git.** Jira ۵ و Redmine ۵: اکوسیستم Atlassian بومی [J-features] و مرور مخزن SCM بومی [RM-repos]. ClickUp/Plane/Taiga/OpenProject ۴: GitHub/GitLab integration موجود (Plane پولی [PLN-pricing]). Tasker ۲: فقط زیرساخت اعتبارنامه/ذخیره، بدون endpoint (`backend/src/integrations/mod.rs:5-7`). Vikunja ۲: بدون یکپارچگی مستند. Planka ۱: هیچ.
5. **سازمان بزرگ، SSO.** Jira ۵: SAML/SCIM با Guard، بلوغ سازمانی کامل [AT-guard]. ClickUp ۴: SAML فقط Enterprise. Plane ۴: SAML/OIDC پولی. OpenProject ۴: SAML/OIDC Enterprise، LDAP رایگان. Vikunja ۳: OIDC رایگان اما بدون SAML/SCIM [VK-openid]. Redmine ۳: LDAP بومی، SAML پلاگین. Tasker ۱: هیچ SSO ندارد — در این سناریو رد می‌شود. Planka ۲: فقط Pro. Taiga ۲: پلاگین OIDC کم‌تکاپو.
6. **تیم Scrum.** Jira ۵: استاندارد صنعت. Taiga ۵: Scrum-first با burndown/تخمین [TG-site]. Plane ۴: Cycles. ClickUp/OpenProject ۴: Sprints موجود. Vikunja ۳: بدون sprint واقعی. Redmine ۳: با پلاگین. Tasker ۲: هیچ مفهوم Sprint/velocity ندارد (فقط شش حالت + داشبورد) — صادقانه ضعیف. Planka ۱.
7. **تیم ساده، فقط Kanban.** Planka ۵: دقیقاً همان. ClickUp/Vikunja/Plane/Tasker ۴: همه کانبان خوب؛ Tasker swimlane+Undo دارد ولی جابه‌جایی داخل ستون ندارد (`KanbanBoard.tsx:231-275`). Taiga ۳: کانبان خوب اما سنگین‌تر. OpenProject ۳. Jira ۲: برای «فقط کانبان» بیش از حد سنگین. Redmine ۲: بدون پلاگین اصلاً کانبان ندارد.

---

# بخش ۷ — شکاف‌های Tasker و نقشه راه پیشنهادی

## ۷.۱ فهرست شکاف‌های واقعی (بر اساس بخش ۳ و ۵)

| # | شکاف | شاهد کد/سند | آیا در نقشه راه موجود است؟ |
|---|---|---|---|
| ۱ | RTL سراسری و UI فارسی و فونت فارسی نداریم | `frontend/index.html` (`lang="en"`)؛ grep بدون نتیجه برای `dir="rtl"`؛ `tailwind.config.ts:13-24` | خیر — در هیچ ADR مستند نیست؛ نزدیک‌ترین شاهد، تست‌های جلالی است که فلسفه «فارسی بودن» را تثبیت می‌کنند (`frontend/e2e/calendar.spec.ts`) |
| ۲ | اعلان ایمیل/push نداریم | `docs/architecture.md` non-goals («e-mail delivery»)؛ `backend/src/notifications.rs` فقط in-app | خیر — فقط به‌عنوان non-goal فاز ۱ ثبت شده |
| ۳ | Webhook خروجی و یکپارچگی چت نداریم | اکشن `send_message` فقط intent معلق است که worker هرگز claim نمی‌کند (`docs/rules-engine.md`؛ `backend/src/rules/dispatcher.rs:14`) | بله — «Messaging Integration Service» در `docs/rules-engine.md` به‌عنوان کار آینده نام برده شده |
| ۴ | یکپارچگی Git ناتمام | `backend/src/integrations/mod.rs:5-7`: intake/adapters/delivery «Tasks 2-5» هنوز نیامده؛ بدون route در `api/openapi.yaml` | بله — Phase 8B Task 1 انجام و Tasks 2-5 در `docs/rules-engine.md:566` به‌عنوان طرح ثبت شده |
| ۵ | UI موتور اتوماسیون نداریم | `frontend/src` فاقد صفحه/کامپوننت rules؛ فقط `schema.d.ts` | جزئی — API و موتور کامل‌اند (ADR نداریم؛ `docs/rules-engine.md` فقط بک‌اند را پوشش می‌دهد) |
| ۶ | S3 پیوست‌ها فقط stub است | `backend/src/attachments.rs:92-108` fail-fast «not available in this build» | ضمنی — enum و config کامل‌اند؛ هیچ ADR تکمیلی وجود ندارد |
| ۷ | SSO/SAML/OIDC نداریم | grep کل بک‌اند بدون oauth/oidc/saml؛ `docs/architecture.md` non-goals («OAuth/OIDC») | خیر — فقط non-goal فاز ۱ |
| ۸ | Sprint/Scrum (iteration، burndown، velocity) نداریم | هیچ migration/module مربوطه؛ `api/openapi.yaml` بدون sprint | خیر |
| ۹ | Time tracking و recurrence نداریم | بدون ستون estimate/spent در migrationها؛ بدون recur | خیر |
| ۱۰ | اپ موبایل نداریم | فقط وب؛ e2e موبایل ۳۷۵px (`frontend/e2e/ux-105.spec.ts:191`) | خیر |
| ۱۱ | تقویم ماهانه/iCal نداریم | بدون `.ics` و بدون صفحه calendar (`frontend/src/pages/`) | خیر |
| ۱۲ | اعمال وابستگی‌ها advisory است | `backend/src/tasks.rs:1911` بدون بررسی blocker | نامشخص — هیچ ADR درباره enforcement نیست |
| ۱۳ | جابه‌جایی کارت داخل ستون Kanban | `KanbanBoard.tsx:231-275` فقط status؛ reorder فقط در List | خیر |
| ۱۴ | WebAuthn، ایمیل بازیابی رمز، audit log عضویت | `docs/security.md` «Known limitations» | ضمنی — به‌عنوان محدودیت شناخته‌شده ثبت شده |
| ۱۵ | Rate limiter پردازه-محل (چندریپلیکا نیست) | `docs/security.md` | خیر |

## ۷.۲ اولویت‌بندی برای بازار هدف (تیم‌های استارتاپی ایرانی)

اولویت ۱ — **RTL سراسری + i18n فارسی + فونت Vazirmatn** (شکاف ۱): ارزان‌ترین مسیر به تمایز بی‌بدیل؛ هیچ رقیبی RTL باکیفیت ندارد و Tasker از نظر تقویم نصف راه را رفته است. بدون این، مزیت جلالی نیم‌کاره می‌ماند.

اولویت ۲ — **اعلان ایمیل + Webhook خروجی** (شکاف ۲ و ۳): تیم‌های واقعی بدون ایمیل زنده نمی‌مانند؛ زیرساخت Outbox از قبل هست (`backend/src/rules/outbox.rs` + dispatcher با lease/fencing) و تحویل `send_message`/ایمیل روی همین ریل، سریع‌ترین برد محصولی است — مستقیماً هم‌راستا با «Messaging Integration Service» ثبت‌شده در `docs/rules-engine.md`.

اولویت ۳ — **تکمیل Phase 8B (Git) و UI قواعد اتوماسیون** (شکاف ۴ و ۵): دو کار نیمه‌تمام موجود؛ تکمیلشان از ساختن ویژگی نو ارزان‌تر است و تیم‌های مهندسی را وارد می‌کند.

اولویت ۴ — **S3 واقعی پیوست‌ها و نمای تقویم** (شکاف ۶ و ۱۱): نگهداشت روزمره؛ S3 فقط یک driver است و تقویم روی همان داده‌های due_date/deadline سوار می‌شود.

اولویت ۵ — **SSO (OIDC دست‌کم)** (شکاف ۷): برای فروش به شرکت‌های بزرگ‌تر ایرانی که Keycloak/OIDC داخلی دارند؛ تا آن زمان، MFA + RBAC پوشش کافی تیم‌های کوچک است.

غیر-اولویت (فعلاً): Sprint/Scrum، time tracking، اپ موبایل — هر سه در رقبا موجودند ولی با بازار هدف فعلی (تیم‌های کوچک غیر-Scrum-سخت ایرانی) رقابت مستقیم ندارند؛ ورود به این‌ها تمرکز محصول را می‌شکند.

---

# بخش ۸ — نتیجه‌گیری

**Tasker امروز کجاست؟** یک محصول self-hosted MIT با پایه مهندسی به‌شدت منظم: قرارداد OpenAPI 3.1 مدیریت‌شده (۹۰ مسیر، CI lint و drift-check)، ۳۱ تست یکپارچگی HTTP × سه موتور دیتابیس، ۹۹ تست e2e با baselines بصری و اسکن axe، ADR‌های منظم، بنچمارک Criterion و اعداد load-test ثبت‌شده (`docs/performance.md`). ویژگی‌محور: چرخه تأیید Reporter یکتا در بازار، MFA رده-سازمانی از روز اول، Goals/OKR، داشبورد KPI و Capacity، موتور اتوماسیون Outbox کامل در بک‌اند، و تقویم جلالی بی‌همتا. ویژگی‌های نیمه‌تمام صادقانه: Git (فقط زیرساخت)، S3 (stub)، اتوماسیون (بی‌UI)، وابستگی (توصیه‌ای). ویژگی‌های غایب پررنگ: ایمیل/Webhook، SSO، RTL/UI فارسی، Sprint، time tracking، موبایل.

**در ۶ ماه آینده کجا می‌تواند باشد؟** اگر اولویت‌های بخش ۷.۲ اجرا شوند، Tasker می‌تواند در شش ماه «تنها گزینه self-hosted با تجربه فارسی واقعاً کامل» باشد — RTL سراسری و i18n گامی مهندسی برای فرانت‌اندی است که از قبل آگاه-به-جلالی ساخته شده؛ ایمیل+Webhook روی Outbox موجود سوار می‌شود؛ و Phase 8B در همان نقشه راه خودش تکمیل می‌شود. با این سه، Tasker از «کد بسیار خوب» به «محصول انتخاب‌شدنی برای تیم‌های ایرانی» عبور می‌کند و در مقایسه مستقیم با Vikunja/Redmine در بازار فارسی برتری ساختاری پیدا می‌کند. در عوض، بدون این‌ها، Tasker در مقایسه با Plane/Vikunja (که هر روز بالغ‌تر می‌شوند) فقط «کد تمیزتر» خواهد بود، نه انتخاب بهتر.

**برای چه کسی بهترین انتخاب است؟ (صریح)** برای تیم ۵ تا ۵۰ نفره‌ای که: داده‌اش باید روی سرور خودش بماند، فرایندش «تحویل → بازبینی → پذیرش» است و می‌خواهد نرم‌افزار آن را تضمین کند، امنیت حساب (MFA، سیاست رمز) برایش مذاکره‌ناپذیر است، تاریخ‌هایش شمسی است، و نمی‌خواهد هیچ پول لایسنس یا سقف صندلی بپردازد.

**برای چه کسی مناسب نیست؟ (صریح)** برای سازمان‌هایی که SSO/SAML لازم دارند؛ برای تیم‌های Scrum رسمی (Sprint/burndown نداریم)؛ برای تیم‌هایی که زندگی‌شان در ایمیل/اسلک می‌گذرد و اعلان داخل اپ کافی نیست؛ برای کسانی که اپ موبایل بومی، ردیابی زمان، یا workflow چند-وضعیتی سفارشی می‌خواهند؛ و — تا اجرای اولویت ۱ — برای هر تیمی که «فارسی بودن رابط» برایش شرط خرید است نه «تقویم فارسی».

---

# بخش ۹ — منابع (References)

تاریخ دسترسی همه منابع وب: **۱۰ نوامبر ۲۰۲۶** (منابع Atlassian که با snapshot آرشیوِ صفحات رسمی استخراج شده‌اند، جدا مشخص شده‌اند).

## ۹.۱ منابع Tasker (کد و مستندات داخلی)

- `README.md`؛ `LICENSE` (MIT)؛ `docker-compose.yml`؛ `deploy/env.example`
- `api/openapi.yaml` (OpenAPI 3.1.0؛ ۹۰ مسیر/۱۲۶ عملیات)؛ `api/README.md`؛ `frontend/src/api/schema.d.ts`
- بک‌اند: `backend/src/{auth,security,mfa,authorization,teams,projects,tasks,comments,activity,notifications,goals,dashboard,workspace,checklists,attachments,guests,admin,users,profile,config,db}.rs`؛ `backend/src/rules/*`؛ `backend/src/integrations/*`؛ `backend/src/http/mod.rs`
- ADRها: `docs/decisions/0001` … `0019` (به‌ویژه 0003, 0004, 0005, 0006, 0007, 0008, 0009, 0010, 0011, 0012, 0014, 0015, 0016, 0017, 0018, 0019) و `docs/adr-0013-project-delete.md`
- مستندات: `docs/architecture.md`، `security.md`، `database-support.md`، `deployment.md`، `configuration.md`، `testing.md`، `performance.md`، `rules-engine.md`
- تست‌ها: `backend/tests/integration.rs` (۳۱ تست ×۳ موتور)؛ `backend/src/**` (۱۳۸ تست واحد)؛ `backend/benches/db.rs` (۸ بنچ)؛ `frontend/src/lib/__tests__/` (۱۵ فایل/~۱۰۳ تست)؛ `frontend/e2e/` (۱۶ spec/۹۹ تست)
- CI: `.github/workflows/ci.yml` (۵ job شامل ماتریس postgres:17-alpine/mysql:8.4)
- فرانت‌اند کلیدی: `frontend/src/lib/{jalali.ts,doran.ts,theme.tsx}`؛ `frontend/src/components/ui/date-field.tsx`؛ `frontend/src/components/app/{TaskTable,KanbanBoard,TaskDetailPanel,NotificationBell,MfaSettings}.tsx`؛ `frontend/src/pages/{ProjectPage,TimelinePage,TaskPage,DashboardPage,WorkloadPage,CapacityPage,GoalsPage,GoalDetailPage,MyTasksPage,AccountPage,TeamGuestsSection,AdminUsersPage}.tsx`؛ `frontend/index.html`؛ `frontend/tailwind.config.ts`

## ۹.۲ منابع ClickUp

- [CU-pricing] https://clickup.com/pricing — پلن‌ها، قیمت‌ها، سهمیه اتوماسیون، جدول گیت ویژگی‌ها، مهمان‌ها
- [CU-onprem] https://feedback.clickup.com/feature-requests/p/self-hosted-enterprise — «on-premise نداریم»
- [CU-hosting] https://help.clickup.com/hc/en-us/articles/15999383444247-Data-hosting
- [CU-subtasks] https://help.clickup.com/hc/en-us/articles/6304431740055-Create-nested-subtasks
- [CU-deps] https://help.clickup.com/hc/en-us/articles/6309155073303-Intro-to-Dependency-Relationships
- [CU-checklists] https://help.clickup.com/hc/en-us/articles/6309942197783-Use-task-checklists
- [CU-github] https://help.clickup.com/hc/en-us/articles/6305771568791-GitHub-integration
- [CU-webhooks] https://developer.clickup.com/docs/webhooks
- [CU-api] https://developer.clickup.com/
- [CU-2fa] https://clickup.com/pricing (ردیف 2FA) و https://help.clickup.com/hc/en-us/articles/6309134405655-Dark-Mode
- [CU-langs] https://help.clickup.com/hc/en-us/articles/6311940941847-Change-your-account-language-region-and-time-and-date-formats
- [CU-rtl] https://feedback.clickup.com/languages/p/rtl-language-support
- [CU-download] https://clickup.com/download
- [CU-attachments] https://help.clickup.com/hc/en-us/articles/6309666546199-Add-attachments-to-tasks
- [CU-notifs] https://help.clickup.com/hc/en-us/articles/6325918957335-Notification-settings
- [CU-pcmag] https://me.pcmag.com/en/productivity/20072/clickup

## ۹.۳ منابع Things 3

- [TH-support] https://culturedcode.com/things/support/articles/2803552/ — خرید یک‌باره/Things Cloud
- [TH-pricing] https://culturedcode.com/things/pricing/
- [TH-features] http://culturedcode.com/things/features/
- [TH-share] https://culturedcode.com/things/support/articles/2803566 — بدون اشتراک real-time
- [TH-urls] https://culturedcode.com/things/support/articles/2803573
- [TH-applescript] https://culturedcode.com/things/support/articles/2803572
- [TH-appstore] https://apps.apple.com/us/app/things-3/id904237743 — زبان‌ها/قیمت
- [TH-ipad] https://apps.apple.com/us/app/things-3-for-ipad/id904244226
- [TH-mac] https://apps.apple.com/us/app/things-3/id904280696?mt=12
- [TH-pcmag] https://me.pcmag.com/en/productivity/29784/things-3
- [MS-things] https://www.macstories.net/reviews/things-3-beauty-and-delight-in-a-task-manager — «No Web API»

## ۹.۴ منابع Jira / Atlassian

- [J-pricing] https://www.atlassian.com/software/jira/pricing (snapshot: http://web.archive.org/web/20251229120218/...)
- [AT-DC] https://www.atlassian.com/licensing/data-center (snapshot: http://web.archive.org/web/20251216181118/...) — پایان فروش DC 2026-03-30، EOL 2029-03-28
- [J-features] https://www.atlassian.com/software/jira/features (snapshot: http://web.archive.org/web/20251222210125/...)
- [J-plans] https://support.atlassian.com/jira-cloud-administration/docs/explore-jira-cloud-plans/
- [J-workflows] https://support.atlassian.com/jira-software-cloud/docs/what-are-jira-workflows/
- [J-subtasks] https://support.atlassian.com/jira-software-cloud/docs/create-a-work-item-and-a-subtask/
- [J-links] https://support.atlassian.com/jira-software-cloud/docs/link-issues/
- [J-dashboards] https://support.atlassian.com/jira-software-cloud/docs/what-is-a-jira-dashboard/
- [J-roadmaps] https://support.atlassian.com/jira-software-cloud/docs/get-started-with-advanced-roadmaps/ — Plans/capacity در Premium+
- [J-approvals] https://support.atlassian.com/jira-service-management-cloud/docs/what-are-approvals/ — تأیید فقط در JSM
- [J-webhooks] https://support.atlassian.com/jira-cloud-administration/docs/manage-webhooks/
- [J-time] https://support.atlassian.com/jira-cloud-administration/docs/configure-time-tracking/
- [J-attach] https://support.atlassian.com/jira-cloud-administration/docs/configure-file-attachments/
- [J-mentions] https://support.atlassian.com/jira-software-cloud/docs/watch-share-and-comment-on-a-work-item/
- [J-mobile] https://support.atlassian.com/jira-software-cloud/docs/use-jira-cloud-on-apple-and-android-devices/
- [J-rest] https://developer.atlassian.com/cloud/jira/platform/rest/v3/intro/ (snapshot: http://web.archive.org/web/20260126195314/...)
- [J-customers] https://support.atlassian.com/jira-service-management-cloud/docs/add-your-customers-so-they-can-raise-requests/
- [AT-guard] https://www.atlassian.com/software/guard/pricing (snapshot: http://web.archive.org/web/20251222210114/...) — SSO/2FA با Guard
- [AT-goals] https://support.atlassian.com/platform-experiences/docs/what-is-a-goal/
- [AT-langs] https://support.atlassian.com/atlassian-account/docs/manage-your-language-preferences/ — بدون فارسی
- [J-dark] https://support.atlassian.com/jira-cloud-android/docs/use-dark-mode/
- [J-pcmag] https://www.pcmag.com/picks/the-best-it-help-desk-software — «overwhelming at first»

## ۹.۵ منابع Plane

- [PLN-gh] https://github.com/makeplane/plane — AGPL-3.0، ستاره‌ها، استک
- [PLN-rel] https://github.com/makeplane/plane/releases — v1.4.3
- [PLN-pricing] https://plane.so/pricing — پلن‌ها، قفل‌ها، مهمان ۱:۵
- [PLN-features] https://plane.so/features — دیدها، Cycles، موبایل، Git/Slack
- [PLN-cycles] https://docs.plane.so/core-concepts/cycles
- [PLN-selfhost] https://developers.plane.so/self-hosting/methods/docker-compose — دو edition، نصب‌کننده تجاری
- [PLN-analytics] https://docs.plane.so/core-concepts/analytics
- [PLN-automations] https://docs.plane.so/automations/overview
- [PLN-webhooks] https://developers.plane.so/dev-tools/intro-webhooks
- [PLN-api] https://developers.plane.so/api-reference/introduction
- [PLN-2fa] https://help.plane.com/en/articles/7970808-how-to-enable-two-factor-authentication-2fa
- [PLN-sso] https://docs.plane.so/authentication/sso
- [PLN-fa-issue] https://github.com/makeplane/plane/issues/9641 — درخواست فارسی/RTL باز
- [PLN-reddit] https://www.reddit.com/r/selfhosted/comments/1oq8y7s/is_planeso_community_edition_good_enough
- [PLN-discussion] https://github.com/orgs/makeplane/discussions/1266

## ۹.۶ منابع Vikunja

- [VK-gh] https://github.com/go-vikunja/vikunja — AGPL-3.0، استک Go+Vue
- [VK-rel] https://github.com/go-vikunja/vikunja/releases — v2.8.0
- [VK-pricing] https://vikunja.io/pricing/ — self-host رایگان مطلق؛ Pro پولی
- [VK-features] https://vikunja.io/features/ — دیدها، relations، تکرارشونده‌ها
- [VK-webhooks] https://vikunja.io/docs/webhooks — HMAC/SSRF
- [VK-openid] https://vikunja.io/docs/openid — OIDC رایگان
- [VK-security] https://vikunja.io/docs/security-hardening — TOTP
- [VK-i18n] https://api.github.com/repos/go-vikunja/vikunja/contents/frontend/src/i18n/lang — fa-IR.json؛ و https://vikunja.io/docs/translations
- [VK-api] https://vikunja.io/docs/api-documentation — Swagger/OpenAPI 3.1/CLI
- [VK-pro] https://vikunja.io/docs/pro — time tracking در Pro
- [VK-app] https://api.github.com/repos/go-vikunja/app — اپ Flutter پایه‌ای
- [VK-config] https://vikunja.io/docs/config-options — S3/local attachments
- [VK-gh2521] https://github.com/go-vikunja/vikunja/issues/2521 — time tracking در هسته نیست
- [VK-gh522] https://github.com/go-vikunja/vikunja/issues/522 — پوشش محدود ایمیل

## ۹.۷ منابع Planka

- [PLK-gh] https://api.github.com/repos/plankanban/planka — ستاره‌ها، توضیح دو-لایسنسه
- [PLK-license] https://github.com/plankanban/planka/blob/master/LICENSE.md — Fair Use v1.1 (دیگر MIT نیست)
- [PLK-rel] https://github.com/plankanban/planka/releases — v2.3.1؛ حذف SSO از Community در v2.2.0؛ TOTP
- [PLK-features] https://planka.app/features — جدول Community vs Pro
- [PLK-pricing] https://planka.app/pricing
- [PLK-docs] https://docs.planka.cloud/docs/api-reference/swagger-ui — API/Webhook
- [PLK-locales] https://api.github.com/repos/plankanban/planka/contents/client/src/locales — fa-IR در ۳۵ locale
- [PLK-1640] https://github.com/plankanban/planka/issues/1640 — نارضایتی پردیوارسازی
- [PLK-apps] https://planka.app/apps

## ۹.۸ منابع Taiga

- [TG-site] https://taiga.io/ — ویژگی‌ها و مدل
- [TG-gov] https://community.taiga.io/t/important-announcement-taiga-will-be-run-by-taiga-cloud-services/3112 — انتقال مالکیت ۲۰۲۴، لغو Taiga Next
- [TG-tenzu] https://tenzu.net/en/blog/tenzu-the-tool-created-by-a-team-frustrated-with-it-project-management
- [TG-pricing] https://taiga.io/deployment-pricing-options/
- [TG-docker] https://docs.taiga.io/ — نصب production با Docker؛ https://github.com/taigaio/taiga-docker
- [TG-custom] https://community.taiga.io/t/customisation-for-your-projects/ — وضعیت‌های سفارشی، نقش External
- [TG-timeline] https://taiga.pm/timeline — Timeline = فید فعالیت
- [TG-deps-issue] https://github.com/taigaio/taiga-front/issues/111 — وابستگی‌ها درخواست باز
- [TG-integrations] https://docs.taiga.io/integrations-github.html (و GitLab/Bitbucket/Gogs)
- [TG-webhooks] https://docs.taiga.io/webhooks-configuration.html
- [TG-api] https://docs.taiga.io/api.html
- [TG-mfa] https://community.taiga.io/t/mandatory-mfa-multi-factor-authentication/3805 — MFA نداریم
- [TG-oidc] https://github.com/taigaio (taiga-contrib-oidc-auth)
- [TG-mobile] https://community.taiga.io/t/proposal-mobile-app/257 — بدون اپ رسمی
- [TG-langs] https://community.taiga.io/t/can-i-use-taiga-in-my-language/142 — «۲۰+ زبان»، وضعیت فارسی نامشخص
- [TG-changelog] https://github.com/taigaio/taiga-back/blob/main/CHANGELOG.md — 6.x

## ۹.۹ منابع OpenProject

- [OP-gh] https://github.com/opf/openproject — GPLv3، ستاره‌ها
- [OP-enterprise] https://www.openproject.org/docs/enterprise-guide — فهرست کامل ویژگی‌های Enterprise
- [OP-pricing] https://www.openproject.org/pricing/ — پلن‌ها و قیمت‌ها
- [OP-rel] https://www.openproject.org/docs/release-notes/ — 17.x
- [OP-gantt] https://www.openproject.org/docs/user-guide/gantt-chart/
- [OP-boards] https://www.openproject.org/blog/agile-boards-for-community و https://www.openproject.org/blog/openproject-17-3-release — boards رایگان
- [OP-relations] https://www.openproject.org/docs/user-guide/work-packages/work-package-relations-hierarchies
- [OP-widgets] https://www.openproject.org/docs/user-guide/projects/project-home — داشبوردها
- [OP-2fa] https://www.openproject.org/docs/release-notes/12/12-4-0 — 2FA رایگان
- [OP-saml] https://www.openproject.org/docs/system-admin-guide/authentication/saml/ — SAML Enterprise
- [OP-webhooks] https://www.openproject.org/docs/system-admin-guide/api-and-webhooks
- [OP-api] https://www.openproject.org/docs/api/ — API v3
- [OP-time] https://www.openproject.org/docs/user-guide/time-and-costs
- [OP-translations] https://www.openproject.org/docs/development/concepts/translations — ۴۰+ زبان
- [OP-rtl] https://community.openproject.org/projects/OP/forums/6/topics/17964 — RTL پشتیبانی نمی‌شود
- [OP-docker] https://www.openproject.org/docs/installation-and-operations/installation/docker/
- [OP-mobile] https://www.openproject.org/docs/mobile-app-guide/help-reference/known-limitations
- [OP-feature-request] https://community.openproject.org/journals/288503/diff/description — چک‌لیست درخواست باز
- [OP-reddit] https://www.reddit.com/r/openproject/comments/113mou4 — نقد open-core

## ۹.۱۰ منابع Redmine

- [RM-site] https://www.redmine.org/ — ویژگی‌های هسته، GPLv2
- [RM-gh] https://github.com/redmine/redmine — آینه؛ SVN رسمی
- [RM-install] https://www.redmine.org/projects/redmine/wiki/RedmineInstall — نصب دستی، بدون Docker رسمی
- [RM-issues] https://www.redmine.org/projects/redmine/wiki/redmineissues — زیرtask بی‌نهایت، روابط
- [RM-repos] https://www.redmine.org/projects/redmine/wiki/RedmineRepositories — SCM بومی
- [RM-api] https://www.redmine.org/projects/redmine/wiki/Rest_API
- [RM-ldap] https://www.redmine.org/projects/redmine/wiki/RedmineLDAP
- [RM-locales] https://github.com/redmine/redmine/tree/master/config/locales — fa.yml در هسته
- [RM-rtl] https://www.redmine.org/issues/43515 — rtl.css جزئی
- [RM-themes] https://www.redmine.org/projects/redmine/wiki/theme_list — ~۸۰+ تم
- [RM-agile] https://www.redmineup.com/pages/plugins/agile — Kanban پلاگینی
- [RM-plugins] https://www.redmine.org/plugins — فهرست رسمی پلاگین‌ها
- [RM-saml] https://github.com/alphanodes/redmine_saml و https://www.redmine.org/issues/43352
- [RM-2fa] https://www.redmine.org/plugins/redmine_out_of_band_auth
- [RM-mentions] https://www.redmine.org/issues/13919 و https://www.redmine.org/projects/redmine/wiki/Changelog_5_0 — mention از 5.0
- [RM-trustradius] https://www.trustradius.com/products/redmine/reviews — «dated UI»

---

*پایان گزارش — تهیه‌شده از کد HEAD `7897d55` و منابع رسمی رقبا در تاریخ ۱۰ نوامبر ۲۰۲۶.*
