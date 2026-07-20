// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use http::header::AUTHORIZATION;
use poem_grpc::{ClientConfig, CompressionEncoding, Metadata};

use crate::{
    id,
    modules::{
        common::rustls::RustMailerTls,
        context::Initialize,
        grpc::service::rustmailer_grpc::{
            AppendReplyToDraftRequest, BatchTagRequest, EmailAddress as GrpcEmailAddress,
            ExternalOAuth2Request, GetThreadMessagesRequest, ListMessagesRequest,
            ListThreadsRequest, MessageServiceClient, OAuth2ServiceClient, Recipient,
            SaveDraftRequest, SendDraftRequest, SendEmailRequest, SendMailServiceClient,
            TagAndColor, TemplateSentTestRequest, TemplatesServiceClient, UnifiedSearchRequest,
        },
    },
};

#[tokio::test]
async fn test1() {
    let cfg = ClientConfig::builder()
        .uri("https://localhost:16630")
        .build()
        .unwrap();
    let mut grpc_client = MessageServiceClient::new(cfg);
    grpc_client.set_accept_compressed([CompressionEncoding::GZIP]);
    grpc_client.set_send_compressed(CompressionEncoding::GZIP);

    let mut metadata = Metadata::new();
    metadata.insert(
        AUTHORIZATION,
        format!("Bearer {}", "2mY4irNCahQXeSarHYje1P1W"),
    );

    let request = ListMessagesRequest {
        account_id: id!(64),
        mailbox_name: "INBOX".into(),
        next_page_token: None,
        page_size: 10,
        remote: false,
        desc: true,
    };

    let mut request = poem_grpc::Request::new(request);
    request.metadata_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", "2mY4irNCahQXeSarHYje1P1W"),
    );

    let response = grpc_client.list_messages(request).await.unwrap();

    let paginated = response.into_inner();
    println!("{:#?}", paginated);
}

#[tokio::test]
async fn test2() {
    RustMailerTls::initialize().await.unwrap();

    let cfg = ClientConfig::builder()
        .uri("http://localhost:16630")
        .build()
        .unwrap();
    let mut grpc_client = TemplatesServiceClient::new(cfg);
    grpc_client.set_accept_compressed([CompressionEncoding::GZIP]);
    grpc_client.set_send_compressed(CompressionEncoding::GZIP);

    let request = TemplateSentTestRequest {
        template_id: 5817286801634245,
        account_id: 5737460794141278,
        recipient: "pollybase@zohomail.com".to_string(),
        template_params: None,
    };

    let mut request = poem_grpc::Request::new(request);
    request.metadata_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", "2mY4irNCahQXeSarHYje1P1W"),
    );

    let response = grpc_client.send_test_email(request).await.unwrap();
}

#[tokio::test]
async fn test3() {
    RustMailerTls::initialize().await.unwrap();

    let cfg = ClientConfig::builder()
        .uri("http://localhost:16630")
        .build()
        .unwrap();
    let mut grpc_client = MessageServiceClient::new(cfg);
    grpc_client.set_accept_compressed([CompressionEncoding::GZIP]);
    grpc_client.set_send_compressed(CompressionEncoding::GZIP);

    let request = UnifiedSearchRequest {
        accounts: vec![],
        email: "news@team.semrush.com".into(),
        after: None,
        before: None,
        page: 1,
        page_size: 15,
        desc: true,
    };

    let mut request = poem_grpc::Request::new(request);
    request.metadata_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", "2mY4irNCahQXeSarHYje1P1W"),
    );
    let response = grpc_client.unified_search(request).await.unwrap();
    println!("{:#?}", response.items);
    println!("{:#?}", response.total_items);
}

#[tokio::test]
async fn test4() {
    RustMailerTls::initialize().await.unwrap();

    let cfg = ClientConfig::builder()
        .uri("http://localhost:16630")
        .build()
        .unwrap();
    let mut grpc_client = MessageServiceClient::new(cfg);
    grpc_client.set_accept_compressed([CompressionEncoding::GZIP]);
    grpc_client.set_send_compressed(CompressionEncoding::GZIP);

    let request = ListThreadsRequest {
        account_id: 8869750310191797,
        mailbox_name: "INBOX".into(),
        next_page_token: Some("1".into()),
        page_size: 15,
        remote: false,
        desc: true,
    };

    let mut request = poem_grpc::Request::new(request);
    request.metadata_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", "2mY4irNCahQXeSarHYje1P1W"),
    );
    let response = grpc_client.list_threads(request).await.unwrap();
    println!("{:#?}", response.items);
}

#[tokio::test]
async fn test5() {
    RustMailerTls::initialize().await.unwrap();

    let cfg = ClientConfig::builder()
        .uri("http://localhost:16630")
        .build()
        .unwrap();
    let mut grpc_client = MessageServiceClient::new(cfg);
    grpc_client.set_accept_compressed([CompressionEncoding::GZIP]);
    grpc_client.set_send_compressed(CompressionEncoding::GZIP);

    let request = GetThreadMessagesRequest {
        account_id: 6606017263301165,
        thread_id: "1572863359614161".into(),
        remote: None,
    };

    let mut request = poem_grpc::Request::new(request);
    request.metadata_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", "2mY4irNCahQXeSarHYje1P1W"),
    );
    let response = grpc_client.get_thread_messages(request).await.unwrap();
    println!("{:#?}", response.items);
}

#[tokio::test]
async fn test6() {
    RustMailerTls::initialize().await.unwrap();

    let cfg = ClientConfig::builder()
        .uri("http://localhost:16630")
        .build()
        .unwrap();
    let mut grpc_client = MessageServiceClient::new(cfg);
    grpc_client.set_accept_compressed([CompressionEncoding::GZIP]);
    grpc_client.set_send_compressed(CompressionEncoding::GZIP);

    let request = AppendReplyToDraftRequest {
        account_id: 6637484689546669,
        mailbox_name: Some("INBOX".into()),
        id: "395".into(),
        preview: None,
        text: Some("hello world.".into()),
        html: None,
        draft_folder_path: Some("[Gmail]/Drafts".into()),
        reply_all: None,
    };

    let mut request = poem_grpc::Request::new(request);
    request.metadata_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", "2mY4irNCahQXeSarHYje1P1W"),
    );
    grpc_client.append_reply_to_draft(request).await.unwrap();
}

#[tokio::test]
async fn test7() {
    RustMailerTls::initialize().await.unwrap();

    let cfg = ClientConfig::builder()
        .uri("http://localhost:16630")
        .build()
        .unwrap();
    let mut grpc_client = OAuth2ServiceClient::new(cfg);
    grpc_client.set_accept_compressed([CompressionEncoding::GZIP]);
    grpc_client.set_send_compressed(CompressionEncoding::GZIP);

    let request = ExternalOAuth2Request {
        account_id: 211386635081531,
        oauth2_id: None,
        access_token: Some("ya29.a0AS3H6Nw6CPT0PaS5ma2P3LJlCYUQ4uA9SaSf7Wd8L6s86NU2p9VfoEXOWnwQUr0LbU6t0ZyYh2SoI7xbokfmJy3VUx39jUGvb31jXzPSsoE41lINxi2OBht0Oe6cjoMU8sebtNj8UFQUE_aFDgaL3YB1EbqTWZ4VGSG1q676mQaCgYKAWASARQSFQHGX2MihHST2SJe5KYZnvun2dohPg0177".into()),
        refresh_token: None,
    };

    let mut request = poem_grpc::Request::new(request);
    request.metadata_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", "2mY4irNCahQXeSarHYje1P1W"),
    );
    grpc_client
        .upsert_external_o_auth2_token(request)
        .await
        .unwrap();
}

#[tokio::test]
async fn test8() {
    RustMailerTls::initialize().await.unwrap();

    let cfg = ClientConfig::builder()
        .uri("http://localhost:16630")
        .build()
        .unwrap();
    let mut grpc_client = MessageServiceClient::new(cfg);
    grpc_client.set_accept_compressed([CompressionEncoding::GZIP]);
    grpc_client.set_send_compressed(CompressionEncoding::GZIP);

    let request = AppendReplyToDraftRequest {
        account_id: 4391092875701825,
        mailbox_name: Some("INBOX".into()),
        preview: None,
        text: Some("hello world.".into()),
        html: None,
        draft_folder_path: None,
        id: "1970d297da3c2dd2".into(),
        reply_all: None,
    };

    let mut request = poem_grpc::Request::new(request);
    request.metadata_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", "2mY4irNCahQXeSarHYje1P1W"),
    );
    grpc_client.append_reply_to_draft(request).await.unwrap();
}

#[tokio::test]
async fn test9() {
    RustMailerTls::initialize().await.unwrap();
    let cfg = ClientConfig::builder()
        .uri("http://localhost:16630")
        .build()
        .unwrap();
    let mut grpc_client = MessageServiceClient::new(cfg);
    grpc_client.set_accept_compressed([CompressionEncoding::GZIP]);
    grpc_client.set_send_compressed(CompressionEncoding::GZIP);

    let request = BatchTagRequest { 
        account_id: 3815676991897752,
        message_ids: vec!["AQMkADAwATMwMAItNzE0OC1jZTEzLTAwAi0wMAoARgAAA_KUk7xWPSBEntPHShr61lgHAOo9V4GwHndCjf0x1uoIcwUAAAIBDAAAAOo9V4GwHndCjf0x1uoIcwUAAckOKwUAAAA=".into()],
        tags: vec![TagAndColor {
            name: "test_name2".into(),
            graph_color: Some("preset1".into()),
            gmail_color: None
        }],
        action: 2,
        mailbox_name: Some("INBOX".into()),
        auto_create_tags: Some(true),
    };

    let mut request = poem_grpc::Request::new(request);
    request.metadata_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", "0ZRTSl2WhTOUQYMCgSm45i1o"),
    );
    grpc_client.tag_messages(request).await.unwrap();
}

// ── Draft save + send tests ──

const RECIPIENT_EMAIL: &str = "rustmailer.git@gmail.com";
const AUTH_TOKEN: &str = "2mY4irNCahQXeSarHYje1P1W";

fn recipient() -> Recipient {
    Recipient {
        to: vec![GrpcEmailAddress {
            name: None,
            address: RECIPIENT_EMAIL.into(),
        }],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        template_params: None,
        send_at: None,
    }
}

fn random_subject(label: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("Draft test [{}] - {}", label, ts)
}

fn send_mail_client() -> SendMailServiceClient {
    let cfg = ClientConfig::builder()
        .uri("http://localhost:16630")
        .build()
        .unwrap();
    let mut grpc_client = SendMailServiceClient::new(cfg);
    grpc_client.set_accept_compressed([CompressionEncoding::GZIP]);
    grpc_client.set_send_compressed(CompressionEncoding::GZIP);
    grpc_client
}

fn auth_request<T>(request: T) -> poem_grpc::Request<T> {
    let mut req = poem_grpc::Request::new(request);
    req.metadata_mut()
        .insert(AUTHORIZATION, format!("Bearer {}", AUTH_TOKEN));
    req
}

#[tokio::test]
async fn test_save_and_send_draft_imap() {
    RustMailerTls::initialize().await.unwrap();

    const ACCOUNT_ID: u64 = 6696853957389495;
    let client = send_mail_client();

    // 1. Save draft
    let email = SendEmailRequest {
        from: None,
        recipients: vec![recipient()],
        subject: Some(random_subject("IMAP")),
        text: Some("This is a test draft from IMAP account.".into()),
        html: None,
        preview: None,
        eml: None,
        template_id: None,
        attachments: vec![],
        headers: Default::default(),
        send_control: None,
    };

    let save_req = SaveDraftRequest {
        account_id: ACCOUNT_ID,
        request: Some(email),
    };

    let response = client
        .save_draft(auth_request(save_req))
        .await
        .expect("IMAP save_draft should succeed");
    let draft = response.into_inner();
    println!("IMAP draft saved: id={}, folder={}", draft.id, draft.draft_folder);
    assert!(!draft.id.is_empty(), "draft id should not be empty");
    assert!(!draft.draft_folder.is_empty(), "draft folder should not be empty");
    assert!(draft.draft_id.is_none(), "IMAP should not have draft_id");

    // 2. Send draft
    let send_req = SendDraftRequest {
        account_id: ACCOUNT_ID,
        id: draft.id.clone(),
    };

    client
        .send_draft(auth_request(send_req))
        .await
        .expect("IMAP send_draft should succeed");
    println!("IMAP draft sent successfully: id={}", draft.id);
}

#[tokio::test]
async fn test_save_and_send_draft_gmail() {
    RustMailerTls::initialize().await.unwrap();

    const ACCOUNT_ID: u64 = 6095192688691414;
    let client = send_mail_client();

    // 1. Save draft
    let email = SendEmailRequest {
        from: None,
        recipients: vec![recipient()],
        subject: Some(random_subject("Gmail")),
        text: Some("This is a test draft from Gmail account.".into()),
        html: None,
        preview: None,
        eml: None,
        template_id: None,
        attachments: vec![],
        headers: Default::default(),
        send_control: None,
    };

    let save_req = SaveDraftRequest {
        account_id: ACCOUNT_ID,
        request: Some(email),
    };

    let response = client
        .save_draft(auth_request(save_req))
        .await
        .expect("Gmail save_draft should succeed");
    let draft = response.into_inner();
    println!(
        "Gmail draft saved: id={}, draft_id={:?}, folder={}",
        draft.id, draft.draft_id, draft.draft_folder
    );
    assert!(!draft.id.is_empty(), "message id should not be empty");
    assert!(!draft.draft_folder.is_empty(), "draft folder should not be empty");
    assert!(
        draft.draft_id.is_some(),
        "Gmail should populate draft_id"
    );

    // 2. Send draft — use draft_id for Gmail
    let send_req = SendDraftRequest {
        account_id: ACCOUNT_ID,
        id: draft.draft_id.unwrap(),
    };

    client
        .send_draft(auth_request(send_req))
        .await
        .expect("Gmail send_draft should succeed");
    println!("Gmail draft sent successfully");
}

#[tokio::test]
async fn test_save_and_send_draft_outlook() {
    RustMailerTls::initialize().await.unwrap();

    const ACCOUNT_ID: u64 = 3815676991897752;
    let client = send_mail_client();

    // 1. Save draft
    let email = SendEmailRequest {
        from: None,
        recipients: vec![recipient()],
        subject: Some(random_subject("Outlook")),
        text: Some("This is a test draft from Outlook account.".into()),
        html: None,
        preview: None,
        eml: None,
        template_id: None,
        attachments: vec![],
        headers: Default::default(),
        send_control: None,
    };

    let save_req = SaveDraftRequest {
        account_id: ACCOUNT_ID,
        request: Some(email),
    };

    let response = client
        .save_draft(auth_request(save_req))
        .await
        .expect("Outlook save_draft should succeed");
    let draft = response.into_inner();
    println!(
        "Outlook draft saved: id={}, folder={}",
        draft.id, draft.draft_folder
    );
    assert!(!draft.id.is_empty(), "message id should not be empty");
    assert!(!draft.draft_folder.is_empty(), "draft folder should not be empty");
    assert!(draft.draft_id.is_none(), "Outlook should not have draft_id");

    // 2. Send draft — use id for Graph API
    let send_req = SendDraftRequest {
        account_id: ACCOUNT_ID,
        id: draft.id.clone(),
    };

    client
        .send_draft(auth_request(send_req))
        .await
        .expect("Outlook send_draft should succeed");
    println!("Outlook draft sent successfully: id={}", draft.id);
}
