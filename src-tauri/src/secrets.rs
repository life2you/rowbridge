//! macOS 钥匙串封装。
//!
//! 密码绝不落盘、绝不进日志。service 固定为 bundle id 派生值，
//! account 用连接 id，这样同一条连接改密码不会残留多条记录。

const SERVICE: &str = "com.rowbridge.app.mysql";

fn entry(id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, id).map_err(|e| format!("访问钥匙串失败: {e}"))
}

pub fn set_password(id: &str, password: &str) -> Result<(), String> {
    if password.is_empty() {
        return delete_password(id);
    }
    entry(id)?
        .set_password(password)
        .map_err(|e| format!("写入钥匙串失败: {e}"))
}

pub fn get_password(id: &str) -> Option<String> {
    entry(id).ok()?.get_password().ok()
}

pub fn delete_password(id: &str) -> Result<(), String> {
    match entry(id)?.delete_credential() {
        Ok(()) => Ok(()),
        // 记录本来就不存在，视为成功
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("删除钥匙串记录失败: {e}")),
    }
}
