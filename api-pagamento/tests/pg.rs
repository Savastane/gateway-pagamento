// Teste de integracao do PgStore. Requer DATABASE_URL (Postgres real).
//   DATABASE_URL=postgres://postgres:postgres@localhost:5435/gw cargo test -p api-pagamento --test pg -- --ignored
use api_pagamento::models::*;
use api_pagamento::store::{NewPayment, PgStore, Store};

fn database_url() -> String {
    std::env::var("DATABASE_URL").expect("DATABASE_URL nao definido")
}

fn new_payment() -> NewPayment {
    NewPayment {
        merchant_id: "m-pg".to_string(),
        gateway: "fake".to_string(),
        method: "pix".to_string(),
        amount_minor: 12990,
        currency: "brl".to_string(),
        idempotency_key: Some("pg-k-1".to_string()),
        metadata: serde_json::json!({"customer": {"name": "Ana"}}),
    }
}

#[tokio::test]
#[ignore]
async fn pgstore_full_lifecycle() {
    let store = PgStore::connect(&database_url()).await.expect("connect");

    // idempotencia: begin => New; replay => mesma resposta; hash diverso => Conflict
    match store.idem_begin("pg-k-1", "m-pg", "hash-a").await.unwrap() {
        IdemBegin::New => {}
        other => panic!("esperava New, veio {other:?}"),
    }
    store
        .idem_finish("pg-k-1", "m-pg", &serde_json::json!({"ok": true}), 200)
        .await
        .unwrap();
    match store.idem_begin("pg-k-1", "m-pg", "hash-a").await.unwrap() {
        IdemBegin::Replay(body, code) => {
            assert_eq!(code, 200);
            assert_eq!(body["ok"], true);
        }
        other => panic!("esperava Replay, veio {other:?}"),
    }
    match store.idem_begin("pg-k-1", "m-pg", "hash-b").await.unwrap() {
        IdemBegin::Conflict => {}
        other => panic!("esperava Conflict, veio {other:?}"),
    }

    // insert + finalize
    let payment = store.insert_payment(&new_payment()).await.unwrap();
    assert_eq!(payment.status, PENDING);
    let pix = serde_json::json!({"emv_qr_code": "EMV", "expires_at": 1});
    let payment = store
        .finalize_create(payment.id, REQUIRES_ACTION, "ext_pg_1", Some(&pix))
        .await
        .unwrap();
    assert_eq!(payment.status, REQUIRES_ACTION);
    assert_eq!(payment.external_id.as_deref(), Some("ext_pg_1"));
    assert_eq!(payment.metadata["pix_details"]["emv_qr_code"], "EMV");

    // find_by_external + apply_event
    let found = store
        .find_by_external("fake", "ext_pg_1")
        .await
        .unwrap()
        .expect("find_by_external");
    assert_eq!(found.id, payment.id);

    // transicao invalida (REQUIRES_ACTION -> REFUNDED) nao aplica
    let same = store
        .apply_event("fake", "ext_pg_1", REFUNDED, &serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(same.status, REQUIRES_ACTION);

    let paid = store
        .apply_event("fake", "ext_pg_1", PAID, &serde_json::json!({"evt": 1}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(paid.status, PAID);

    // refund parcial + total
    match store.reserve_refund(payment.id, 1000, "r-1").await.unwrap() {
        ReserveOutcome::Reserved { refund_id, amount } => {
            assert_eq!(amount, 1000);
            let status = store.settle_refund(refund_id, "re_1", true).await.unwrap();
            assert_eq!(status.as_deref(), Some(PARTIALLY_REFUNDED));
        }
        other => panic!("esperava Reserved, veio {other:?}"),
    }
    // excede o restante
    match store
        .reserve_refund(payment.id, 999999, "r-2")
        .await
        .unwrap()
    {
        ReserveOutcome::ExceedsPaid { remaining } => assert_eq!(remaining, 11990),
        other => panic!("esperava ExceedsPaid, veio {other:?}"),
    }
    // total (0 = restante)
    match store.reserve_refund(payment.id, 0, "r-3").await.unwrap() {
        ReserveOutcome::Reserved { refund_id, amount } => {
            assert_eq!(amount, 11990);
            let status = store.settle_refund(refund_id, "re_2", true).await.unwrap();
            assert_eq!(status.as_deref(), Some(REFUNDED));
        }
        other => panic!("esperava Reserved, veio {other:?}"),
    }
    // pagamento totalmente estornado: estado invalido
    match store.reserve_refund(payment.id, 0, "r-4").await.unwrap() {
        ReserveOutcome::InvalidState { status } => assert_eq!(status, REFUNDED),
        other => panic!("esperava InvalidState, veio {other:?}"),
    }

    // eventos registrados
    let pool = sqlx::PgPool::connect(&database_url()).await.unwrap();
    let events: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM payment_events WHERE payment_id = $1")
            .bind(payment.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(events >= 3, "esperava >=3 eventos, achou {events}");
}
