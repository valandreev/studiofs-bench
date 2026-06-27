# Code Review: Linear SFS-569 (Define Benchmark Configuration and Domain Model)

**Дата проведения:** 2026-06-27  
**Crate:** `studiofs-bench`  
**Использованное руководство:** `rust-best-practices` (на основе Apollo GraphQL Rust Best Practices)

---

## Введение

Был проведен детальный обзор кода, добавленного в рамках задачи **Linear SFS-569** (коммиты на ветке `feature/sfs-569-define-benchmark-configuration-and-domain-model`). В ходе ревью анализировались файлы:
- [src/lib.rs](file:///E:/dev/studiofs-bench/src/lib.rs)
- [src/main.rs](file:///E:/dev/studiofs-bench/src/main.rs)
- [tests/config_model.rs](file:///E:/dev/studiofs-bench/tests/config_model.rs)
- [tests/smoke.rs](file:///E:/dev/studiofs-bench/tests/smoke.rs)
- [Cargo.toml](file:///E:/dev/studiofs-bench/Cargo.toml)

Код написан аккуратно, соответствует редакции Rust 2024 (MSRV 1.95), хорошо документирован doc-комментариями и успешно проходит `cargo clippy`. Тем не менее, были найдены несколько областей для улучшения, недоработок и потенциальных ошибок.

---

## 1. Ошибки и критические недоработки

### 1.1. Отсутствие реализации `std::error::Error` для `ConfigError`
* **Файл:** [src/lib.rs#L173-L196](file:///E:/dev/studiofs-bench/src/lib.rs#L173-L196)
* **Проблема:** Перечисление `ConfigError` реализует `fmt::Display` вручную, но **не** реализует типаж `std::error::Error`. 
* **Почему это важно:** В экосистеме Rust любой пользовательский тип ошибки должен реализовывать `std::error::Error`. Без этого ошибку невозможно приводить к объекту типажа `Box<dyn std::error::Error>`, использовать в цепочках ошибок с `std::error::Error::source` или эргономично оборачивать с помощью макросов логирования/других библиотек.
* **Как исправить:**
  ```rust
  impl std::error::Error for ConfigError {}
  ```

### 1.2. Риск переполнения целого числа (Integer Overflow) при подсчете байтов
* **Файл:** [src/lib.rs#L86-L104](file:///E:/dev/studiofs-bench/src/lib.rs#L86-L104)
* **Проблема:** В реализации `WorkloadSize` функции `megabytes()` и `bytes()` производят обычное умножение:
  ```rust
  pub fn megabytes(self) -> u64 {
      self.gigabytes() * MB_PER_GB
  }

  pub fn bytes(self) -> u64 {
      self.megabytes() * DECIMAL_MB
  }
  ```
  Если пользователь передаст в `WorkloadSize::CustomGb` экстремально большое значение (например, близкое к `u64::MAX`), вызов этих функций приведет к панике в Debug-режиме (из-за встроенных проверок на переполнение) или к некорректному зацикливанию (wrapping) значения в Release-режиме.
* **Как исправить:** Использовать безопасное насыщающее умножение (`saturating_mul`):
  ```rust
  pub fn megabytes(self) -> u64 {
      self.gigabytes().saturating_mul(MB_PER_GB)
  }

  pub fn bytes(self) -> u64 {
      self.megabytes().saturating_mul(DECIMAL_MB)
  }
  ```

---

## 2. Архитектурные недоработки и несогласованности

### 2.1. Избыточное изменяемое поле `throughput_unit` в `BenchmarkConfig`
* **Файл:** [src/lib.rs#L30-L31](file:///E:/dev/studiofs-bench/src/lib.rs#L30-L31), [src/lib.rs#L46](file:///E:/dev/studiofs-bench/src/lib.rs#L46)
* **Проблема:** Поле `throughput_unit: &'static str` жестко инициализируется значением `"MB/s"`.
  1. Если единица измерения пропускной способности всегда статична для данного бенчмарка, хранение ее в каждом экземпляре структуры тратит память (размер указателя, 8 байт).
  2. Так как поле публичное, пользователь может его изменить (например, `config.throughput_unit = "GB/s"`). При этом вся остальная логика расчетов (`bytes()`, `megabytes()`) останется привязанной к мегабайтам и гигабайтам, что создаст рассинхронизацию между реальной логикой и строковым описанием.
  3. Поле никак не валидируется в методе `.validate()`.
* **Рекомендация:**
  - Если единица измерения неизменна, сделать ее ассоциированной константой структуры:
    ```rust
    impl BenchmarkConfig {
        pub const THROUGHPUT_UNIT: &'static str = "MB/s";
    }
    ```
    или возвращать через метод.
  - Если единица измерения должна быть настраиваемой, заменить `&'static str` на типизированное перечисление `ThroughputUnit` (например, `enum ThroughputUnit { MbPerSec, GbPerSec, MiBPerSec, GiBPerSec }`), чтобы валидировать его и пересчитывать пропускную способность корректно.

### 2.2. Проблема кратности размера файла и общего объема нагрузки
* **Файл:** [src/lib.rs#L68-L70](file:///E:/dev/studiofs-bench/src/lib.rs#L68-L70)
* **Проблема:** При конфигурации `FileLayout::FixedFileSizeMb(file_size_mb)` валидация проверяет только то, что размер файла не превышает общий размер нагрузки. Однако, если общий размер нагрузки не делится нацело на размер файла (например, нагрузка 4000 MB, а размер файла 1500 MB), бенчмарк создаст файлы разного размера (два по 1500 MB и один 1000 MB).
* **Рекомендация:** Задокументировать это поведение в doc-комментариях к `FileLayout::FixedFileSizeMb` или добавить в `validate()` проверку на кратность (или предупреждение), если это критично для точности бенчмарка.

---

## 3. Рекомендации по улучшению тестов

### 3.1. Недостаточное покрытие валидатора тестами
* **Файл:** [tests/config_model.rs](file:///E:/dev/studiofs-bench/tests/config_model.rs)
* **Проблема:** 
  1. Тест `default_config_uses_documented_benchmark_contract` проверяет значения по умолчанию, но **не вызывает** метод `.validate()` на дефолтной конфигурации. Важно убедиться, что конфигурация "из коробки" проходит валидацию.
  2. Валидатор проверяет 4 разных ошибочных сценария (пустой путь, нулевой размер нагрузки, нулевой размер файла, превышение размера файла над нагрузкой), но в тестах проверяется только один сценарий (`validate_rejects_fixed_file_layout_larger_than_workload`).
* **Рекомендация:** Добавить тесты на успешную валидацию дефолтной конфигурации и на оставшиеся ошибки:
  ```rust
  #[test]
  fn default_config_is_valid() {
      let config = BenchmarkConfig::for_target(PathBuf::from("E:/bench-target"));
      assert!(config.validate().is_ok());
  }

  #[test]
  fn validate_rejects_empty_target_path() {
      let config = BenchmarkConfig::for_target(PathBuf::new());
      assert_eq!(config.validate(), Err(ConfigError::EmptyTargetPath));
  }

  #[test]
  fn validate_rejects_zero_workload() {
      let mut config = BenchmarkConfig::for_target(PathBuf::from("E:/bench-target"));
      config.workload_size = WorkloadSize::CustomGb(0);
      assert_eq!(config.validate(), Err(ConfigError::ZeroWorkload));
  }

  #[test]
  fn validate_rejects_zero_file_size() {
      let mut config = BenchmarkConfig::for_target(PathBuf::from("E:/bench-target"));
      config.file_layout = FileLayout::FixedFileSizeMb(0);
      assert_eq!(config.validate(), Err(ConfigError::ZeroFileSize));
  }
  ```

### 3.2. Частичная проверка сериализации
* **Файл:** [tests/config_model.rs#L37-L55](file:///E:/dev/studiofs-bench/tests/config_model.rs#L37-L55)
* **Проблема:** В тесте `config_serializes_report_ready_values` проверяются не все поля результирующего JSON-объекта. В частности, поле `file_layout` никак не проверяется после сериализации.
* **Рекомендация:** Добавить проверку сериализации поля `file_layout`, например:
  ```rust
  assert_eq!(value["file_layout"], "single_file");
  ```

---

## 4. Качество кода и стандарты Rust Best Practices

### 4.1. Принудительное документирование библиотеки
* **Файл:** [src/lib.rs](file:///E:/dev/studiofs-bench/src/lib.rs)
* **Рекомендация:** Согласно главе 8 руководства по лучшим практикам ("Enable `#![deny(missing_docs)]` for libraries"), рекомендуется добавить атрибут на уровне модуля библиотеки:
  ```rust
  #![deny(missing_docs)]
  ```
  Это гарантирует, что любые новые структуры, перечисления или публичные функции в будущем не останутся без документации.

### 4.2. Использование внешних библиотек для ошибок
* **Проблема:** Обработка ошибок в библиотеке реализована вручную.
* **Рекомендация:** Для крупного проекта ручная поддержка `Display` и `std::error::Error` для каждого перечисления ошибок может стать избыточной. В будущем рекомендуется добавить в зависимости `thiserror` для декларативного объявления ошибок.
