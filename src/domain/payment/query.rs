#[derive(Default)]
pub struct ListQuery {
    pub paid_filter: Option<ListQueryPaidFilter>,
    pub ack_filter: Option<ListQueryAckFilter>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ListQueryPaidFilter {
    Paid,
    Unpaid,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ListQueryAckFilter {
    Pending,
    Accepted,
}
