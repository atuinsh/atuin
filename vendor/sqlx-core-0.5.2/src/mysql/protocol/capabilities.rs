// https://dev.mysql.com/doc/dev/mysql-server/8.0.12/group__group__cs__capabilities__flags.html
// https://mariadb.com/kb/en/library/connection/#capabilities
bitflags::bitflags! {
    pub struct Capabilities: u64 {
        // [MariaDB] MySQL compatibility
        const MYSQL = 1;

        // [*] Send found rows instead of affected rows in EOF_Packet.
        const FOUND_ROWS = 2;

        // Get all column flags.
        const LONG_FLAG = 4;

        // [*] Database (schema) name can be specified on connect in Handshake Response Packet.
        const CONNECT_WITH_DB = 8;

        // Don't allow database.table.column
        const NO_SCHEMA = 16;

        // [*] Compression protocol supported
        const COMPRESS = 32;

        // Special handling of ODBC behavior.
        const ODBC = 64;

        // Can use LOAD DATA LOCAL
        const LOCAL_FILES = 128;

        // [*] Ignore spaces before '('
        const IGNORE_SPACE = 256;

        // [*] New 4.1+ protocol
        const PROTOCOL_41 = 512;

        // This is an interactive client
        const INTERACTIVE = 1024;

        // Use SSL encryption for this session
        const SSL = 2048;

        // Client knows about transactions
        const TRANSACTIONS = 8192;

        // 4.1+ authentication
        const SECURE_CONNECTION = (1 << 15);

        // Enable/disable multi-statement support for COM_QUERY *and* COM_STMT_PREPARE
        const MULTI_STATEMENTS = (1 << 16);

        // Enable/disable multi-results for COM_QUERY
        const MULTI_RESULTS = (1 << 17);

        // Enable/disable multi-results for COM_STMT_PREPARE
        const PS_MULTI_RESULTS = (1 << 18);

        // Client supports plugin authentication
        const PLUGIN_AUTH = (1 << 19);

        // Client supports connection attributes
        const CONNECT_ATTRS = (1 << 20);

        // Enable authentication response packet to be larger than 255 bytes.
        const PLUGIN_AUTH_LENENC_DATA = (1 << 21);

        // Don't close the connection for a user account with expired password.
        const CAN_HANDLE_EXPIRED_PASSWORDS = (1 << 22);

        // Capable of handling server state change information.
        const SESSION_TRACK = (1 << 23);

        // Client no longer needs EOF_Packet and will use OK_Packet instead.
        const DEPRECATE_EOF = (1 << 24);

        // Support ZSTD protocol compression
        const ZSTD_COMPRESSION_ALGORITHM = (1 << 26);

        // Verify server certificate
        const SSL_VERIFY_SERVER_CERT = (1 << 30);

        // The client can handle optional metadata information in the resultset
        const OPTIONAL_RESULTSET_METADATA = (1 << 25);

        // Don't reset the options after an unsuccessful connect
        const REMEMBER_OPTIONS = (1 << 31);
    }
}
