//! Dono unico do schema do banco: concentra as migrations de todos os
//! servicos (api-pagamento, webhook-pagamento) para que uma unica tabela
//! `_sqlx_migrations` versione tudo. Consumers nao devem embutir suas
//! proprias migrations — chamar `banco::migrate` ou `banco::connect`.

pub use sqlx::PgPool;

/// Aplica as migrations pendentes (idempotente).
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

/// Conecta e ja aplica as migrations.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    connect_with(database_url, 5).await
}

/// Conecta com tamanho de pool configuravel e aplica as migrations.
pub async fn connect_with(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database_url)
        .await?;
    migrate(&pool)
        .await
        .map_err(|e| sqlx::Error::Migrate(Box::new(e)))?;
    Ok(pool)
}
