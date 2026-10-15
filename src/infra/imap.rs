use std::collections::HashSet;

use async_trait::async_trait;
use mail_parser::{MessageParser, MimeHeaders};

use crate::domain::email::{Email, Mailbox, MailboxError, Pdf};

const TEXT_LIMIT: usize = 50_000;
const PDF_LIMIT: usize = 10 * 1024 * 1024;

#[derive(Clone)]
pub struct GmailMailbox {
    host: String,
    user: String,
    password: String,
    label: String,
}

impl GmailMailbox {
    pub fn new(host: String, user: String, password: String, label: String) -> Self {
        Self {
            host,
            user,
            password,
            label,
        }
    }

    fn fetch_new_blocking(&self, known: &HashSet<String>) -> Result<Vec<Email>, MailboxError> {
        let error = |context: &str| {
            let context = context.to_string();
            move |err: imap::Error| MailboxError(format!("{context}: {err}"))
        };
        let client = imap::ClientBuilder::new(self.host.as_str(), 993)
            .connect()
            .map_err(error("connect"))?;
        let mut session = client
            .login(&self.user, &self.password)
            .map_err(|(err, _)| MailboxError(format!("login: {err}")))?;

        let mailbox = session.examine(&self.label).map_err(error("open label"))?;
        if mailbox.exists == 0 {
            let _ = session.logout();
            return Ok(Vec::new());
        }

        let envelopes = session
            .uid_fetch("1:*", "(UID ENVELOPE)")
            .map_err(error("list"))?;
        let new: Vec<(u32, String)> = envelopes
            .iter()
            .filter_map(|fetch| {
                let uid = fetch.uid?;
                let message_id = fetch
                    .envelope()
                    .and_then(|envelope| envelope.message_id.as_ref())
                    .map(|id| String::from_utf8_lossy(id).trim().to_string())
                    .filter(|id| !id.is_empty())
                    .unwrap_or_else(|| format!("<imap-uid-{uid}@{}>", self.label));
                (!known.contains(&message_id)).then_some((uid, message_id))
            })
            .collect();

        let mut emails = Vec::with_capacity(new.len());
        for (uid, message_id) in new {
            let fetches = session
                .uid_fetch(uid.to_string(), "BODY.PEEK[]")
                .map_err(error("fetch"))?;
            match fetches.iter().find_map(|fetch| fetch.body()) {
                Some(raw) => emails.push(email_from_raw(message_id, raw)),
                None => tracing::warn!(uid, %message_id, "email body missing from fetch"),
            }
        }

        let _ = session.logout();
        Ok(emails)
    }
}

#[async_trait]
impl Mailbox for GmailMailbox {
    async fn fetch_new(&self, known: &HashSet<String>) -> Result<Vec<Email>, MailboxError> {
        let mailbox = self.clone();
        let known = known.clone();
        tokio::task::spawn_blocking(move || mailbox.fetch_new_blocking(&known))
            .await
            .map_err(|err| MailboxError(format!("imap task failed: {err}")))?
    }
}

fn email_from_raw(message_id: String, raw: &[u8]) -> Email {
    let Some(message) = MessageParser::default().parse(raw) else {
        tracing::warn!(%message_id, "unparsable email");
        return Email {
            message_id,
            from: String::new(),
            subject: String::new(),
            text: String::new(),
            pdfs: Vec::new(),
        };
    };

    let from = message
        .from()
        .and_then(|from| from.first())
        .and_then(|address| address.address())
        .unwrap_or_default()
        .to_string();
    let subject = message.subject().unwrap_or_default().to_string();

    let text = truncate(
        message.body_text(0).unwrap_or_default().into_owned(),
        TEXT_LIMIT,
    );
    let mut pdfs = Vec::new();
    let mut pdf_bytes = 0;
    let mut attachments = Vec::new();
    for attachment in message.attachments() {
        let name = attachment
            .attachment_name()
            .unwrap_or("attachment")
            .to_string();
        let is_pdf = attachment.content_type().is_some_and(|ct| {
            ct.subtype()
                .is_some_and(|sub| sub.eq_ignore_ascii_case("pdf"))
        }) || name.to_lowercase().ends_with(".pdf");
        let bytes = attachment.contents();
        if !is_pdf {
            attachments.push(format!("{name} (skipped, not a pdf)"));
        } else if pdf_bytes + bytes.len() > PDF_LIMIT {
            attachments.push(format!("{name} (skipped, over the 10 MB limit)"));
        } else {
            attachments.push(format!("{name} ({} KB)", bytes.len() / 1024));
            pdf_bytes += bytes.len();
            pdfs.push(Pdf {
                name,
                bytes: bytes.to_vec(),
            });
        }
    }
    tracing::info!(
        %message_id,
        body_chars = text.chars().count(),
        ?attachments,
        "read email"
    );
    tracing::debug!(%message_id, %text, "email body sent to the parser");

    Email {
        message_id,
        from,
        subject,
        text,
        pdfs,
    }
}

fn truncate(mut text: String, limit: usize) -> String {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn pdf_with_text(line: &str) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 72 712 Td ({line}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
        }
        let xref = pdf.len();
        pdf.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in offsets {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        pdf.into_bytes()
    }

    fn base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn mime(attachments: &[(&str, &str, Vec<u8>)]) -> Vec<u8> {
        let mut raw = String::from(
            "From: Biuro Rachunkowe <ksiegowa@biuro.pl>\r\n\
             Subject: =?UTF-8?Q?Faktura_za_wrzesie=C5=84?=\r\n\
             Message-ID: <m1@biuro.pl>\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
             --b\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\n\
             Do zapłaty 307,50 zł.\r\n",
        );
        for (name, content_type, bytes) in attachments {
            raw.push_str(&format!(
                "--b\r\nContent-Type: {content_type}; name=\"{name}\"\r\n\
                 Content-Disposition: attachment; filename=\"{name}\"\r\n\
                 Content-Transfer-Encoding: base64\r\n\r\n{}\r\n",
                base64(bytes)
            ));
        }
        raw.push_str("--b--\r\n");
        raw.into_bytes()
    }

    #[test]
    fn reads_sender_subject_body_and_keeps_pdfs_as_files() {
        let pdf = pdf_with_text("Numer konta 61109010140000071219812874");
        let raw = mime(&[
            ("faktura.pdf", "application/pdf", pdf.clone()),
            ("logo.png", "image/png", vec![0x89, b'P', b'N', b'G']),
        ]);

        let email = email_from_raw("<m1@biuro.pl>".into(), &raw);

        assert_eq!(email.from, "ksiegowa@biuro.pl");
        assert_eq!(email.subject, "Faktura za wrzesień");
        assert!(email.text.contains("Do zapłaty 307,50 zł."));
        assert_eq!(
            email.pdfs,
            vec![Pdf {
                name: "faktura.pdf".into(),
                bytes: pdf
            }]
        );
    }

    #[test]
    fn pdfs_over_the_size_limit_are_skipped() {
        let big = vec![b'x'; PDF_LIMIT - 10];
        let raw = mime(&[
            ("big.pdf", "application/pdf", big),
            ("second.pdf", "application/pdf", b"%PDF-1.4 small".to_vec()),
        ]);

        let email = email_from_raw("<m1@biuro.pl>".into(), &raw);

        assert_eq!(email.pdfs.len(), 1);
        assert_eq!(email.pdfs[0].name, "big.pdf");
    }

    #[test]
    fn text_is_capped_on_a_char_boundary() {
        let text = "ż".repeat(30_000);
        let capped = truncate(text, TEXT_LIMIT);
        assert!(capped.len() <= TEXT_LIMIT);
        assert!(capped.chars().all(|c| c == 'ż'));
    }
}
