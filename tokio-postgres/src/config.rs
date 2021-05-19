//! Connection configuration.

#[cfg(feature = "runtime")]
use crate::connect::connect;
use crate::connect_raw::connect_raw;
#[cfg(not(target_arch = "wasm32"))]
use crate::keepalive::KeepaliveConfig;
#[cfg(feature = "runtime")]
use crate::tls::MakeTlsConnect;
use crate::tls::TlsConnect;
#[cfg(feature = "runtime")]
use crate::Socket;
use crate::{Client, Connection, Error};
use std::borrow::Cow;
#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::str;
use std::str::FromStr;
use std::time::Duration;
use std::{error, fmt, iter, mem};
use tokio::io::{AsyncRead, AsyncWrite};

/// Properties required of a session.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetSessionAttrs {
    /// No special properties are required.
    Any,
    /// The session must allow writes.
    ReadWrite,
}

/// TLS configuration.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SslMode {
    /// Do not use TLS.
    Disable,
    /// Attempt to connect with TLS but allow sessions without.
    Prefer,
    /// Require the use of TLS.
    Require,
    /// Require the use of TLS.
    VerifyCa,
    /// Require the use of TLS.
    VerifyFull,
}

/// Channel binding configuration.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChannelBinding {
    /// Do not use channel binding.
    Disable,
    /// Attempt to use channel binding but allow sessions without.
    Prefer,
    /// Require the use of channel binding.
    Require,
}

/// Replication mode configuration.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReplicationMode {
    /// Physical replication.
    Physical,
    /// Logical replication.
    Logical,
}

/// A host specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    /// A TCP hostname.
    Tcp(String),
    /// A path to a directory containing the server's Unix socket.
    ///
    /// This variant is only available on Unix platforms.
    #[cfg(unix)]
    Unix(PathBuf),
}

/// Connection configuration.
///
/// Configuration can be parsed from libpq-style connection strings. These strings come in two formats:
///
/// # Key-Value
///
/// This format consists of space-separated key-value pairs. Values which are either the empty string or contain
/// whitespace should be wrapped in `'`. `'` and `\` characters should be backslash-escaped.
///
/// ## Keys
///
/// * `user` - The username to authenticate with. Required.
/// * `password` - The password to authenticate with.
/// * `dbname` - The name of the database to connect to. Defaults to the username.
/// * `options` - Command line options used to configure the server.
/// * `application_name` - Sets the `application_name` parameter on the server.
/// * `sslcert` - Location of the client SSL certificate file.
/// * `sslkey` - Location for the secret key file used for the client certificate.
/// * `sslmode` - Controls usage of TLS. If set to `disable`, TLS will not be used. If set to `prefer`, TLS will be used
///     if available, but not used otherwise. If set to `require`, `verify-ca`, or `verify-full`, TLS will be forced to
///     be used. Defaults to `prefer`.
/// * `sslrootcert` - Location of SSL certificate authority (CA) certificate.
/// * `host` - The host to connect to. On Unix platforms, if the host starts with a `/` character it is treated as the
///     path to the directory containing Unix domain sockets. Otherwise, it is treated as a hostname. Multiple hosts
///     can be specified, separated by commas. Each host will be tried in turn when connecting. Required if connecting
///     with the `connect` method.
/// * `port` - The port to connect to. Multiple ports can be specified, separated by commas. The number of ports must be
///     either 1, in which case it will be used for all hosts, or the same as the number of hosts. Defaults to 5432 if
///     omitted or the empty string.
/// * `connect_timeout` - The time limit in seconds applied to each socket-level connection attempt. Note that hostnames
///     can resolve to multiple IP addresses, and this limit is applied to each address. Defaults to no timeout.
/// * `tcp_user_timeout` - The time limit that transmitted data may remain unacknowledged before a connection is forcibly closed.
///     This is ignored for Unix domain socket connections. It is only supported on systems where TCP_USER_TIMEOUT is available
///     and will default to the system default if omitted or set to 0; on other systems, it has no effect.
/// * `keepalives` - Controls the use of TCP keepalive. A value of 0 disables keepalive and nonzero integers enable it.
///     This option is ignored when connecting with Unix sockets. Defaults to on.
/// * `keepalives_idle` - The number of seconds of inactivity after which a keepalive message is sent to the server.
///     This option is ignored when connecting with Unix sockets. Defaults to 2 hours.
/// * `keepalives_interval` - The time interval between TCP keepalive probes.
///     This option is ignored when connecting with Unix sockets.
/// * `keepalives_retries` - The maximum number of TCP keepalive probes that will be sent before dropping a connection.
///     This option is ignored when connecting with Unix sockets.
/// * `target_session_attrs` - Specifies requirements of the session. If set to `read-write`, the client will check that
///     the `transaction_read_write` session parameter is set to `on`. This can be used to connect to the primary server
///     in a database cluster as opposed to the secondary read-only mirrors. Defaults to `all`.
/// * `channel_binding` - Controls usage of channel binding in the authentication process. If set to `disable`, channel
///     binding will not be used. If set to `prefer`, channel binding will be used if available, but not used otherwise.
///     If set to `require`, the authentication process will fail if channel binding is not used. Defaults to `prefer`.
///
/// ## Examples
///
/// ```not_rust
/// host=localhost user=postgres connect_timeout=10 keepalives=0
/// ```
///
/// ```not_rust
/// host=/var/lib/postgresql,localhost port=1234 user=postgres password='password with spaces'
/// ```
///
/// ```not_rust
/// host=host1,host2,host3 port=1234,,5678 user=postgres target_session_attrs=read-write
/// ```
///
/// # Url
///
/// This format resembles a URL with a scheme of either `postgres://` or `postgresql://`. All components are optional,
/// and the format accepts query parameters for all of the key-value pairs described in the section above. Multiple
/// host/port pairs can be comma-separated. Unix socket paths in the host section of the URL should be percent-encoded,
/// as the path component of the URL specifies the database name.
///
/// ## Examples
///
/// ```not_rust
/// postgresql://user@localhost
/// ```
///
/// ```not_rust
/// postgresql://user:password@%2Fvar%2Flib%2Fpostgresql/mydb?connect_timeout=10
/// ```
///
/// ```not_rust
/// postgresql://user@host1:1234,host2,host3:5678?target_session_attrs=read-write
/// ```
///
/// ```not_rust
/// postgresql:///mydb?user=user&host=/var/lib/postgresql
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub(crate) user: Option<String>,
    pub(crate) password: Option<Vec<u8>>,
    pub(crate) dbname: Option<String>,
    pub(crate) options: Option<String>,
    pub(crate) application_name: Option<String>,
    pub(crate) ssl_cert: Option<Vec<u8>>,
    pub(crate) ssl_key: Option<Vec<u8>>,
    pub(crate) ssl_mode: SslMode,
    pub(crate) ssl_root_cert: Option<Vec<u8>>,
    pub(crate) host: Vec<Host>,
    pub(crate) port: Vec<u16>,
    pub(crate) connect_timeout: Option<Duration>,
    pub(crate) tcp_user_timeout: Option<Duration>,
    pub(crate) keepalives: bool,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) keepalive_config: KeepaliveConfig,
    pub(crate) target_session_attrs: TargetSessionAttrs,
    pub(crate) channel_binding: ChannelBinding,
    pub(crate) replication_mode: Option<ReplicationMode>,
}

impl Default for Config {
    fn default() -> Config {
        Config::new()
    }
}

impl Config {
    /// Creates a new configuration.
    pub fn new() -> Config {
        Config {
            user: None,
            password: None,
            dbname: None,
            options: None,
            application_name: None,
            ssl_cert: None,
            ssl_key: None,
            ssl_mode: SslMode::Prefer,
            ssl_root_cert: None,
            host: vec![],
            port: vec![],
            connect_timeout: None,
            tcp_user_timeout: None,
            keepalives: true,
            #[cfg(not(target_arch = "wasm32"))]
            keepalive_config: KeepaliveConfig {
                idle: Duration::from_secs(2 * 60 * 60),
                interval: None,
                retries: None,
            },
            target_session_attrs: TargetSessionAttrs::Any,
            channel_binding: ChannelBinding::Prefer,
            replication_mode: None,
        }
    }

    /// Sets the user to authenticate with.
    ///
    /// Required.
    pub fn user(&mut self, user: &str) -> &mut Config {
        self.user = Some(user.to_string());
        self
    }

    /// Gets the user to authenticate with, if one has been configured with
    /// the `user` method.
    pub fn get_user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Sets the password to authenticate with.
    pub fn password<T>(&mut self, password: T) -> &mut Config
    where
        T: AsRef<[u8]>,
    {
        self.password = Some(password.as_ref().to_vec());
        self
    }

    /// Gets the password to authenticate with, if one has been configured with
    /// the `password` method.
    pub fn get_password(&self) -> Option<&[u8]> {
        self.password.as_deref()
    }

    /// Sets the name of the database to connect to.
    ///
    /// Defaults to the user.
    pub fn dbname(&mut self, dbname: &str) -> &mut Config {
        self.dbname = Some(dbname.to_string());
        self
    }

    /// Gets the name of the database to connect to, if one has been configured
    /// with the `dbname` method.
    pub fn get_dbname(&self) -> Option<&str> {
        self.dbname.as_deref()
    }

    /// Sets command line options used to configure the server.
    pub fn options(&mut self, options: &str) -> &mut Config {
        self.options = Some(options.to_string());
        self
    }

    /// Gets the command line options used to configure the server, if the
    /// options have been set with the `options` method.
    pub fn get_options(&self) -> Option<&str> {
        self.options.as_deref()
    }

    /// Sets the value of the `application_name` runtime parameter.
    pub fn application_name(&mut self, application_name: &str) -> &mut Config {
        self.application_name = Some(application_name.to_string());
        self
    }

    /// Gets the value of the `application_name` runtime parameter, if it has
    /// been set with the `application_name` method.
    pub fn get_application_name(&self) -> Option<&str> {
        self.application_name.as_deref()
    }

    /// Sets the client SSL certificate in PEM format.
    ///
    /// Defaults to `None`.
    pub fn ssl_cert(&mut self, ssl_cert: &[u8]) -> &mut Config {
        self.ssl_cert = Some(ssl_cert.into());
        self
    }

    /// Gets the location of the client SSL certificate in PEM format.
    pub fn get_ssl_cert(&self) -> Option<&[u8]> {
        self.ssl_cert.as_deref()
    }

    /// Sets the client SSL key in PEM format.
    ///
    /// Defaults to `None`.
    pub fn ssl_key(&mut self, ssl_key: &[u8]) -> &mut Config {
        self.ssl_key = Some(ssl_key.into());
        self
    }

    /// Gets the client SSL key in PEM format.
    pub fn get_ssl_key(&self) -> Option<&[u8]> {
        self.ssl_key.as_deref()
    }

    /// Sets the SSL configuration.
    ///
    /// Defaults to `prefer`.
    pub fn ssl_mode(&mut self, ssl_mode: SslMode) -> &mut Config {
        self.ssl_mode = ssl_mode;
        self
    }

    /// Gets the SSL configuration.
    pub fn get_ssl_mode(&self) -> SslMode {
        self.ssl_mode
    }

    /// Sets the SSL certificate authority (CA) certificate in PEM format.
    ///
    /// Defaults to `None`.
    pub fn ssl_root_cert(&mut self, ssl_root_cert: &[u8]) -> &mut Config {
        self.ssl_root_cert = Some(ssl_root_cert.into());
        self
    }

    /// Gets the SSL certificate authority (CA) certificate in PEM format.
    pub fn get_ssl_root_cert(&self) -> Option<&[u8]> {
        self.ssl_root_cert.as_deref()
    }

    /// Adds a host to the configuration.
    ///
    /// Multiple hosts can be specified by calling this method multiple times, and each will be tried in order. On Unix
    /// systems, a host starting with a `/` is interpreted as a path to a directory containing Unix domain sockets.
    pub fn host(&mut self, host: &str) -> &mut Config {
        #[cfg(unix)]
        {
            if host.starts_with('/') {
                return self.host_path(host);
            }
        }

        self.host.push(Host::Tcp(host.to_string()));
        self
    }

    /// Gets the hosts that have been added to the configuration with `host`.
    pub fn get_hosts(&self) -> &[Host] {
        &self.host
    }

    /// Adds a Unix socket host to the configuration.
    ///
    /// Unlike `host`, this method allows non-UTF8 paths.
    #[cfg(unix)]
    pub fn host_path<T>(&mut self, host: T) -> &mut Config
    where
        T: AsRef<Path>,
    {
        self.host.push(Host::Unix(host.as_ref().to_path_buf()));
        self
    }

    /// Adds a port to the configuration.
    ///
    /// Multiple ports can be specified by calling this method multiple times. There must either be no ports, in which
    /// case the default of 5432 is used, a single port, in which it is used for all hosts, or the same number of ports
    /// as hosts.
    pub fn port(&mut self, port: u16) -> &mut Config {
        self.port.push(port);
        self
    }

    /// Gets the ports that have been added to the configuration with `port`.
    pub fn get_ports(&self) -> &[u16] {
        &self.port
    }

    /// Sets the timeout applied to socket-level connection attempts.
    ///
    /// Note that hostnames can resolve to multiple IP addresses, and this timeout will apply to each address of each
    /// host separately. Defaults to no limit.
    pub fn connect_timeout(&mut self, connect_timeout: Duration) -> &mut Config {
        self.connect_timeout = Some(connect_timeout);
        self
    }

    /// Gets the connection timeout, if one has been set with the
    /// `connect_timeout` method.
    pub fn get_connect_timeout(&self) -> Option<&Duration> {
        self.connect_timeout.as_ref()
    }

    /// Sets the TCP user timeout.
    ///
    /// This is ignored for Unix domain socket connections. It is only supported on systems where
    /// TCP_USER_TIMEOUT is available and will default to the system default if omitted or set to 0;
    /// on other systems, it has no effect.
    pub fn tcp_user_timeout(&mut self, tcp_user_timeout: Duration) -> &mut Config {
        self.tcp_user_timeout = Some(tcp_user_timeout);
        self
    }

    /// Gets the TCP user timeout, if one has been set with the
    /// `user_timeout` method.
    pub fn get_tcp_user_timeout(&self) -> Option<&Duration> {
        self.tcp_user_timeout.as_ref()
    }

    /// Controls the use of TCP keepalive.
    ///
    /// This is ignored for Unix domain socket connections. Defaults to `true`.
    pub fn keepalives(&mut self, keepalives: bool) -> &mut Config {
        self.keepalives = keepalives;
        self
    }

    /// Reports whether TCP keepalives will be used.
    pub fn get_keepalives(&self) -> bool {
        self.keepalives
    }

    /// Sets the amount of idle time before a keepalive packet is sent on the connection.
    ///
    /// This is ignored for Unix domain sockets, or if the `keepalives` option is disabled. Defaults to 2 hours.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn keepalives_idle(&mut self, keepalives_idle: Duration) -> &mut Config {
        self.keepalive_config.idle = keepalives_idle;
        self
    }

    /// Gets the configured amount of idle time before a keepalive packet will
    /// be sent on the connection.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn get_keepalives_idle(&self) -> Duration {
        self.keepalive_config.idle
    }

    /// Sets the time interval between TCP keepalive probes.
    /// On Windows, this sets the value of the tcp_keepalive struct’s keepaliveinterval field.
    ///
    /// This is ignored for Unix domain sockets, or if the `keepalives` option is disabled.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn keepalives_interval(&mut self, keepalives_interval: Duration) -> &mut Config {
        self.keepalive_config.interval = Some(keepalives_interval);
        self
    }

    /// Gets the time interval between TCP keepalive probes.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn get_keepalives_interval(&self) -> Option<Duration> {
        self.keepalive_config.interval
    }

    /// Sets the maximum number of TCP keepalive probes that will be sent before dropping a connection.
    ///
    /// This is ignored for Unix domain sockets, or if the `keepalives` option is disabled.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn keepalives_retries(&mut self, keepalives_retries: u32) -> &mut Config {
        self.keepalive_config.retries = Some(keepalives_retries);
        self
    }

    /// Gets the maximum number of TCP keepalive probes that will be sent before dropping a connection.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn get_keepalives_retries(&self) -> Option<u32> {
        self.keepalive_config.retries
    }

    /// Sets the requirements of the session.
    ///
    /// This can be used to connect to the primary server in a clustered database rather than one of the read-only
    /// secondary servers. Defaults to `Any`.
    pub fn target_session_attrs(
        &mut self,
        target_session_attrs: TargetSessionAttrs,
    ) -> &mut Config {
        self.target_session_attrs = target_session_attrs;
        self
    }

    /// Gets the requirements of the session.
    pub fn get_target_session_attrs(&self) -> TargetSessionAttrs {
        self.target_session_attrs
    }

    /// Sets the channel binding behavior.
    ///
    /// Defaults to `prefer`.
    pub fn channel_binding(&mut self, channel_binding: ChannelBinding) -> &mut Config {
        self.channel_binding = channel_binding;
        self
    }

    /// Gets the channel binding behavior.
    pub fn get_channel_binding(&self) -> ChannelBinding {
        self.channel_binding
    }

    /// Set replication mode.
    pub fn replication_mode(&mut self, replication_mode: ReplicationMode) -> &mut Config {
        self.replication_mode = Some(replication_mode);
        self
    }

    /// Get replication mode.
    pub fn get_replication_mode(&self) -> Option<ReplicationMode> {
        self.replication_mode
    }

    fn param(&mut self, key: &str, value: &str) -> Result<(), Error> {
        match key {
            "user" => {
                self.user(value);
            }
            "password" => {
                self.password(value);
            }
            "dbname" => {
                self.dbname(value);
            }
            "options" => {
                self.options(value);
            }
            "application_name" => {
                self.application_name(value);
            }
            "sslcert" => match std::fs::read(&value) {
                Ok(contents) => {
                    self.ssl_cert(&contents);
                }
                Err(_) => {
                    return Err(Error::config_parse(Box::new(InvalidValue("sslcert"))));
                }
            },
            "sslkey" => match std::fs::read(&value) {
                Ok(contents) => {
                    self.ssl_key(&contents);
                }
                Err(_) => {
                    return Err(Error::config_parse(Box::new(InvalidValue("sslkey"))));
                }
            },
            "sslmode" => {
                let mode = match value {
                    "disable" => SslMode::Disable,
                    "prefer" => SslMode::Prefer,
                    "require" => SslMode::Require,
                    "verify-ca" => SslMode::VerifyCa,
                    "verify-full" => SslMode::VerifyFull,
                    _ => return Err(Error::config_parse(Box::new(InvalidValue("sslmode")))),
                };
                self.ssl_mode(mode);
            }
            "sslrootcert" => match std::fs::read(&value) {
                Ok(contents) => {
                    self.ssl_root_cert(&contents);
                }
                Err(_) => {
                    return Err(Error::config_parse(Box::new(InvalidValue("sslrootcert"))));
                }
            },
            "host" => {
                for host in value.split(',') {
                    self.host(host);
                }
            }
            "port" => {
                for port in value.split(',') {
                    let port = if port.is_empty() {
                        5432
                    } else {
                        port.parse()
                            .map_err(|_| Error::config_parse(Box::new(InvalidValue("port"))))?
                    };
                    self.port(port);
                }
            }
            "connect_timeout" => {
                let timeout = value
                    .parse::<i64>()
                    .map_err(|_| Error::config_parse(Box::new(InvalidValue("connect_timeout"))))?;
                if timeout > 0 {
                    self.connect_timeout(Duration::from_secs(timeout as u64));
                }
            }
            "tcp_user_timeout" => {
                let timeout = value
                    .parse::<i64>()
                    .map_err(|_| Error::config_parse(Box::new(InvalidValue("tcp_user_timeout"))))?;
                if timeout > 0 {
                    self.tcp_user_timeout(Duration::from_secs(timeout as u64));
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            "keepalives" => {
                let keepalives = value
                    .parse::<u64>()
                    .map_err(|_| Error::config_parse(Box::new(InvalidValue("keepalives"))))?;
                self.keepalives(keepalives != 0);
            }
            #[cfg(not(target_arch = "wasm32"))]
            "keepalives_idle" => {
                let keepalives_idle = value
                    .parse::<i64>()
                    .map_err(|_| Error::config_parse(Box::new(InvalidValue("keepalives_idle"))))?;
                if keepalives_idle > 0 {
                    self.keepalives_idle(Duration::from_secs(keepalives_idle as u64));
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            "keepalives_interval" => {
                let keepalives_interval = value.parse::<i64>().map_err(|_| {
                    Error::config_parse(Box::new(InvalidValue("keepalives_interval")))
                })?;
                if keepalives_interval > 0 {
                    self.keepalives_interval(Duration::from_secs(keepalives_interval as u64));
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            "keepalives_retries" => {
                let keepalives_retries = value.parse::<u32>().map_err(|_| {
                    Error::config_parse(Box::new(InvalidValue("keepalives_retries")))
                })?;
                self.keepalives_retries(keepalives_retries);
            }
            "target_session_attrs" => {
                let target_session_attrs = match value {
                    "any" => TargetSessionAttrs::Any,
                    "read-write" => TargetSessionAttrs::ReadWrite,
                    _ => {
                        return Err(Error::config_parse(Box::new(InvalidValue(
                            "target_session_attrs",
                        ))));
                    }
                };
                self.target_session_attrs(target_session_attrs);
            }
            "channel_binding" => {
                let channel_binding = match value {
                    "disable" => ChannelBinding::Disable,
                    "prefer" => ChannelBinding::Prefer,
                    "require" => ChannelBinding::Require,
                    _ => {
                        return Err(Error::config_parse(Box::new(InvalidValue(
                            "channel_binding",
                        ))))
                    }
                };
                self.channel_binding(channel_binding);
            }
            "replication" => {
                let mode = match value {
                    "off" => None,
                    "true" => Some(ReplicationMode::Physical),
                    "database" => Some(ReplicationMode::Logical),
                    _ => return Err(Error::config_parse(Box::new(InvalidValue("replication")))),
                };
                if let Some(mode) = mode {
                    self.replication_mode(mode);
                }
            }
            key => {
                return Err(Error::config_parse(Box::new(UnknownOption(
                    key.to_string(),
                ))));
            }
        }

        Ok(())
    }

    /// Opens a connection to a PostgreSQL database.
    ///
    /// Requires the `runtime` Cargo feature (enabled by default).
    #[cfg(feature = "runtime")]
    pub async fn connect<T>(&self, tls: T) -> Result<(Client, Connection<Socket, T::Stream>), Error>
    where
        T: MakeTlsConnect<Socket>,
    {
        connect(tls, self).await
    }

    /// Connects to a PostgreSQL database over an arbitrary stream.
    ///
    /// All of the settings other than `user`, `password`, `dbname`, `options`, and `application_name` name are ignored.
    pub async fn connect_raw<S, T>(
        &self,
        stream: S,
        tls: T,
    ) -> Result<(Client, Connection<S, T::Stream>), Error>
    where
        S: AsyncRead + AsyncWrite + Unpin,
        T: TlsConnect<S>,
    {
        connect_raw(stream, tls, self).await
    }
}

impl FromStr for Config {
    type Err = Error;

    fn from_str(s: &str) -> Result<Config, Error> {
        match UrlParser::parse(s)? {
            Some(config) => Ok(config),
            None => Parser::parse(s),
        }
    }
}

// Omit password from debug output
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Redaction {}
        impl fmt::Debug for Redaction {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "_")
            }
        }

        let mut config_dbg = &mut f.debug_struct("Config");
        config_dbg = config_dbg
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| Redaction {}))
            .field("dbname", &self.dbname)
            .field("options", &self.options)
            .field("application_name", &self.application_name)
            .field("ssl_cert", &self.ssl_cert)
            .field("ssl_key", &self.ssl_key)
            .field("ssl_mode", &self.ssl_mode)
            .field("ssl_root_cert", &self.ssl_root_cert)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("connect_timeout", &self.connect_timeout)
            .field("tcp_user_timeout", &self.tcp_user_timeout)
            .field("keepalives", &self.keepalives);

        #[cfg(not(target_arch = "wasm32"))]
        {
            config_dbg = config_dbg
                .field("keepalives_idle", &self.keepalive_config.idle)
                .field("keepalives_interval", &self.keepalive_config.interval)
                .field("keepalives_retries", &self.keepalive_config.retries);
        }

        config_dbg
            .field("target_session_attrs", &self.target_session_attrs)
            .field("channel_binding", &self.channel_binding)
            .field("replication", &self.replication_mode)
            .finish()
    }
}

#[derive(Debug)]
struct UnknownOption(String);

impl fmt::Display for UnknownOption {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "unknown option `{}`", self.0)
    }
}

impl error::Error for UnknownOption {}

#[derive(Debug)]
struct InvalidValue(&'static str);

impl fmt::Display for InvalidValue {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "invalid value for option `{}`", self.0)
    }
}

impl error::Error for InvalidValue {}

struct Parser<'a> {
    s: &'a str,
    it: iter::Peekable<str::CharIndices<'a>>,
}

impl<'a> Parser<'a> {
    fn parse(s: &'a str) -> Result<Config, Error> {
        let mut parser = Parser {
            s,
            it: s.char_indices().peekable(),
        };

        let mut config = Config::new();

        while let Some((key, value)) = parser.parameter()? {
            config.param(key, &value)?;
        }

        Ok(config)
    }

    fn skip_ws(&mut self) {
        self.take_while(char::is_whitespace);
    }

    fn take_while<F>(&mut self, f: F) -> &'a str
    where
        F: Fn(char) -> bool,
    {
        let start = match self.it.peek() {
            Some(&(i, _)) => i,
            None => return "",
        };

        loop {
            match self.it.peek() {
                Some(&(_, c)) if f(c) => {
                    self.it.next();
                }
                Some(&(i, _)) => return &self.s[start..i],
                None => return &self.s[start..],
            }
        }
    }

    fn eat(&mut self, target: char) -> Result<(), Error> {
        match self.it.next() {
            Some((_, c)) if c == target => Ok(()),
            Some((i, c)) => {
                let m = format!(
                    "unexpected character at byte {}: expected `{}` but got `{}`",
                    i, target, c
                );
                Err(Error::config_parse(m.into()))
            }
            None => Err(Error::config_parse("unexpected EOF".into())),
        }
    }

    fn eat_if(&mut self, target: char) -> bool {
        match self.it.peek() {
            Some(&(_, c)) if c == target => {
                self.it.next();
                true
            }
            _ => false,
        }
    }

    fn keyword(&mut self) -> Option<&'a str> {
        let s = self.take_while(|c| match c {
            c if c.is_whitespace() => false,
            '=' => false,
            _ => true,
        });

        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    fn value(&mut self) -> Result<String, Error> {
        let value = if self.eat_if('\'') {
            let value = self.quoted_value()?;
            self.eat('\'')?;
            value
        } else {
            self.simple_value()?
        };

        Ok(value)
    }

    fn simple_value(&mut self) -> Result<String, Error> {
        let mut value = String::new();

        while let Some(&(_, c)) = self.it.peek() {
            if c.is_whitespace() {
                break;
            }

            self.it.next();
            if c == '\\' {
                if let Some((_, c2)) = self.it.next() {
                    value.push(c2);
                }
            } else {
                value.push(c);
            }
        }

        if value.is_empty() {
            return Err(Error::config_parse("unexpected EOF".into()));
        }

        Ok(value)
    }

    fn quoted_value(&mut self) -> Result<String, Error> {
        let mut value = String::new();

        while let Some(&(_, c)) = self.it.peek() {
            if c == '\'' {
                return Ok(value);
            }

            self.it.next();
            if c == '\\' {
                if let Some((_, c2)) = self.it.next() {
                    value.push(c2);
                }
            } else {
                value.push(c);
            }
        }

        Err(Error::config_parse(
            "unterminated quoted connection parameter value".into(),
        ))
    }

    fn parameter(&mut self) -> Result<Option<(&'a str, String)>, Error> {
        self.skip_ws();
        let keyword = match self.keyword() {
            Some(keyword) => keyword,
            None => return Ok(None),
        };
        self.skip_ws();
        self.eat('=')?;
        self.skip_ws();
        let value = self.value()?;

        Ok(Some((keyword, value)))
    }
}

// This is a pretty sloppy "URL" parser, but it matches the behavior of libpq, where things really aren't very strict
struct UrlParser<'a> {
    s: &'a str,
    config: Config,
}

impl<'a> UrlParser<'a> {
    fn parse(s: &'a str) -> Result<Option<Config>, Error> {
        let s = match Self::remove_url_prefix(s) {
            Some(s) => s,
            None => return Ok(None),
        };

        let mut parser = UrlParser {
            s,
            config: Config::new(),
        };

        parser.parse_credentials()?;
        parser.parse_host()?;
        parser.parse_path()?;
        parser.parse_params()?;

        Ok(Some(parser.config))
    }

    fn remove_url_prefix(s: &str) -> Option<&str> {
        for prefix in &["postgres://", "postgresql://"] {
            if let Some(stripped) = s.strip_prefix(prefix) {
                return Some(stripped);
            }
        }

        None
    }

    fn take_until(&mut self, end: &[char]) -> Option<&'a str> {
        match self.s.find(end) {
            Some(pos) => {
                let (head, tail) = self.s.split_at(pos);
                self.s = tail;
                Some(head)
            }
            None => None,
        }
    }

    fn take_all(&mut self) -> &'a str {
        mem::take(&mut self.s)
    }

    fn eat_byte(&mut self) {
        self.s = &self.s[1..];
    }

    fn parse_credentials(&mut self) -> Result<(), Error> {
        let creds = match self.take_until(&['@']) {
            Some(creds) => creds,
            None => return Ok(()),
        };
        self.eat_byte();

        let mut it = creds.splitn(2, ':');
        let user = self.decode(it.next().unwrap())?;
        self.config.user(&user);

        if let Some(password) = it.next() {
            let password = Cow::from(percent_encoding::percent_decode(password.as_bytes()));
            self.config.password(password);
        }

        Ok(())
    }

    fn parse_host(&mut self) -> Result<(), Error> {
        let host = match self.take_until(&['/', '?']) {
            Some(host) => host,
            None => self.take_all(),
        };

        if host.is_empty() {
            return Ok(());
        }

        for chunk in host.split(',') {
            let (host, port) = if chunk.starts_with('[') {
                let idx = match chunk.find(']') {
                    Some(idx) => idx,
                    None => return Err(Error::config_parse(InvalidValue("host").into())),
                };

                let host = &chunk[1..idx];
                let remaining = &chunk[idx + 1..];
                let port = if let Some(port) = remaining.strip_prefix(':') {
                    Some(port)
                } else if remaining.is_empty() {
                    None
                } else {
                    return Err(Error::config_parse(InvalidValue("host").into()));
                };

                (host, port)
            } else {
                let mut it = chunk.splitn(2, ':');
                (it.next().unwrap(), it.next())
            };

            self.host_param(host)?;
            let port = self.decode(port.unwrap_or("5432"))?;
            self.config.param("port", &port)?;
        }

        Ok(())
    }

    fn parse_path(&mut self) -> Result<(), Error> {
        if !self.s.starts_with('/') {
            return Ok(());
        }
        self.eat_byte();

        let dbname = match self.take_until(&['?']) {
            Some(dbname) => dbname,
            None => self.take_all(),
        };

        if !dbname.is_empty() {
            self.config.dbname(&self.decode(dbname)?);
        }

        Ok(())
    }

    fn parse_params(&mut self) -> Result<(), Error> {
        if !self.s.starts_with('?') {
            return Ok(());
        }
        self.eat_byte();

        while !self.s.is_empty() {
            let key = match self.take_until(&['=']) {
                Some(key) => self.decode(key)?,
                None => return Err(Error::config_parse("unterminated parameter".into())),
            };
            self.eat_byte();

            let value = match self.take_until(&['&']) {
                Some(value) => {
                    self.eat_byte();
                    value
                }
                None => self.take_all(),
            };

            if key == "host" {
                self.host_param(value)?;
            } else {
                let value = self.decode(value)?;
                self.config.param(&key, &value)?;
            }
        }

        Ok(())
    }

    #[cfg(unix)]
    fn host_param(&mut self, s: &str) -> Result<(), Error> {
        let decoded = Cow::from(percent_encoding::percent_decode(s.as_bytes()));
        if decoded.first() == Some(&b'/') {
            self.config.host_path(OsStr::from_bytes(&decoded));
        } else {
            let decoded = str::from_utf8(&decoded).map_err(|e| Error::config_parse(Box::new(e)))?;
            self.config.host(decoded);
        }

        Ok(())
    }

    #[cfg(not(unix))]
    fn host_param(&mut self, s: &str) -> Result<(), Error> {
        let s = self.decode(s)?;
        self.config.param("host", &s)
    }

    fn decode(&self, s: &'a str) -> Result<Cow<'a, str>, Error> {
        percent_encoding::percent_decode(s.as_bytes())
            .decode_utf8()
            .map_err(|e| Error::config_parse(e.into()))
    }
}
