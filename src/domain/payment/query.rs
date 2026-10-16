pub struct ListQuery {
    /// None means all
    pub paid_filter: Option<ListQueryPaidFilter>,
    /// None means all
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
