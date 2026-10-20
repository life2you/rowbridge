#!/usr/bin/env bash
# RowBridge e2e 测试：双 MySQL 实例上验证引擎行为。
#
# 用法：./tests/e2e/run.sh
set -uo pipefail

cd "$(dirname "$0")/../.."

src_mysql() { docker exec rowbridge-src mysql -uroot -proot123 -N --raw -e "$1" "${2:-db_a}" 2>/dev/null; }
dst_mysql() { docker exec rowbridge-dst mysql -uroot -proot123 -N --raw -e "$1" "${2:-db_b}" 2>/dev/null; }
dst_exec()  { docker exec rowbridge-dst mysql -uroot -proot123 -e "$1" 2>/dev/null; }

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n    期望 %s，实际 %s\n' "$1" "$2" "$3"; }
eq()   { [ "$2" = "$3" ] && ok "$1" || bad "$1" "$2" "$3"; }

BIN=./target/debug/rowbridge-cli
COMMON="--src-host 127.0.0.1 --src-port 3307 --src-user root --src-pass root123 \
        --dst-host 127.0.0.1 --dst-port 3308 --dst-user root --dst-pass root123"

cargo build -p rowbridge-core --bins -q || { echo "构建失败"; exit 1; }

wait_mysql() {
  for _ in $(seq 1 60); do
    if docker exec rowbridge-src mysqladmin ping -uroot -proot123 -h127.0.0.1 -s >/dev/null 2>&1 &&
       docker exec rowbridge-dst mysqladmin ping -uroot -proot123 -h127.0.0.1 -s >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  echo "MySQL 未就绪"; exit 1
}

echo "== 准备环境 =="
(cd tests/e2e && docker compose up -d >/dev/null 2>&1)
wait_mysql
docker exec -i rowbridge-src mysql -uroot -proot123 < tests/e2e/seed-src.sql 2>/dev/null
dst_exec "DROP DATABASE IF EXISTS db_b; CREATE DATABASE db_b"
echo "  环境就绪"

sync() { $BIN $COMMON sync --from db_a --to db_b "$@" 2>&1; }

echo
echo "== 1. 空表：建表成功且 0 行 =="
OUT=$(sync --table empty_t)
dst_tables=$(dst_mysql "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='db_b' AND table_name='empty_t'")
dst_rows=$(dst_mysql "SELECT COUNT(*) FROM empty_t")
eq "目标表已创建" "1" "$dst_tables"
eq "行数为 0" "0" "$dst_rows"

echo
echo "== 2. 主键有空洞：不漏行 =="
sync --table sparse >/dev/null
eq "行数与主键集合" "1,100,100000,1000000" "$(dst_mysql "SELECT GROUP_CONCAT(id ORDER BY id) FROM sparse")"

echo
echo "== 3. 主键含 0 和负数 =="
sync --table signed_pk >/dev/null
eq "负数与零都在" "-100,-1,0,1,50" "$(dst_mysql "SELECT GROUP_CONCAT(id ORDER BY id) FROM signed_pk")"

echo
echo "== 4. bigint unsigned 接近 u64::MAX：不溢出 =="
sync --table big_unsigned >/dev/null
eq "u64 最大值无损（主键）" "18446744073709551615" "$(dst_mysql "SELECT id FROM big_unsigned WHERE v='u64max'")"

echo
echo "== 5. 全数据类型逐列一致 =="
sync --table types_all >/dev/null
Q="SELECT MD5(CONCAT_WS('#', id,c_tinyint,c_ubigint,c_decimal,c_float,c_double,HEX(c_bit),c_char,
   c_varchar,length(c_text),HEX(c_blob),c_json,c_date,c_time,c_datetime,c_timestamp,c_year,
   c_enum,c_set,HEX(c_binary),HEX(c_varbinary),HEX(c_geometry))) FROM types_all"
eq "整行指纹一致" "$(src_mysql "$Q")" "$(dst_mysql "$Q")"

echo
echo "== 6. 分页不丢行：10000 行 / 每批 137 =="
dst_exec "DROP TABLE IF EXISTS db_b.bulk"
sync --table bulk --batch 137 --no-adaptive >/dev/null
eq "行数" "10000" "$(dst_mysql "SELECT COUNT(*) FROM bulk")"
eq "校验和" "$(src_mysql "SELECT SUM(n) FROM bulk")" "$(dst_mysql "SELECT SUM(n) FROM bulk")"

echo
echo "== 7. 自适应批大小不得提前结束（回归） =="
dst_exec "DROP TABLE IF EXISTS db_b.bulk"
sync --table bulk >/dev/null
eq "自适应下仍取满 10000 行" "10000" "$(dst_mysql "SELECT COUNT(*) FROM bulk")"

echo
echo "== 8. UPSERT 幂等 + 目标端多余行保留 =="
sync --table simple >/dev/null
dst_exec "UPDATE db_b.simple SET name='TAMPERED' WHERE id=3"
sync --table simple >/dev/null
eq "被篡改的值已覆盖回源值" "carol" "$(dst_mysql "SELECT name FROM simple WHERE id=3")"
dst_exec "INSERT INTO db_b.simple VALUES (999,'extra',1,NOW())"
sync --table simple >/dev/null
eq "首版不做 DELETE 同步，多余行保留" "1,2,3,4,5,999" "$(dst_mysql "SELECT GROUP_CONCAT(id ORDER BY id) FROM simple")"
dst_exec "DELETE FROM db_b.simple WHERE id=999"

echo
echo "== 9. 断点续跑：--end-id 分段 + resume =="
dst_exec "DROP TABLE IF EXISTS db_b.bulk"
sync --table bulk --batch 500 --end-id 5000 --no-adaptive >/dev/null
eq "第一段停在 5000" "5000" "$(dst_mysql "SELECT COUNT(*) FROM bulk")"
sync --table bulk --batch 500 --cursor resume --no-adaptive >/dev/null
eq "续传后补齐 10000" "10000" "$(dst_mysql "SELECT COUNT(*) FROM bulk")"
eq "续传后校验和一致" "$(src_mysql "SELECT SUM(n) FROM bulk")" "$(dst_mysql "SELECT SUM(n) FROM bulk")"

echo
echo "== 10. 不支持的主键被拦截 =="
OUT=$(sync --table composite_pk --table no_pk --table str_pk --dry-run)
echo "$OUT" | grep -q '复合主键' && ok "复合主键已提示" || bad "复合主键已提示" "含『复合主键』" "未找到"
echo "$OUT" | grep -q '无主键' && ok "无主键已提示" || bad "无主键已提示" "含『无主键』" "未找到"
echo "$OUT" | grep -q '主键 id 类型为 varchar' && ok "字符串主键已提示" || bad "字符串主键已提示" "含类型说明" "未找到"
n_sync=$(echo "$OUT" | grep -c '^\[OK')
eq "无一被判定可同步" "0" "$n_sync"

echo
echo "== 11. 目标缺必填列 → 预检阻塞 =="
dst_exec "ALTER TABLE db_b.simple ADD COLUMN must_have VARCHAR(20) NOT NULL"
OUT=$(sync --table simple --dry-run)
echo "$OUT" | grep -q '必定失败' && ok "已识别缺失列" || bad "已识别缺失列" "含『必定失败』" "未找到"
echo "$OUT" | grep -q '^\[SKIP' && ok "被标记为不可同步" || bad "被标记为不可同步" "SKIP" "OK"
dst_exec "ALTER TABLE db_b.simple DROP COLUMN must_have"

echo
echo "== 12. 生成列被跳过，物化列值正确 =="
sync --table generated_col >/dev/null
eq "生成列随源计算" "30.00" "$(dst_mysql "SELECT total FROM generated_col WHERE id=1")"

echo
echo "== 13. 外键在建表时被剥离 =="
sync --table parent_t --table child_t >/dev/null
fk=$(dst_mysql "SELECT COUNT(*) FROM information_schema.table_constraints
                WHERE table_schema='db_b' AND table_name='child_t' AND constraint_type='FOREIGN KEY'")
eq "目标表无外键约束" "0" "$fk"
eq "子表数据完整" "2" "$(dst_mysql "SELECT COUNT(*) FROM child_t")"

echo
echo "== 14. 多表并行 =="
dst_exec "DROP DATABASE IF EXISTS db_b; CREATE DATABASE db_b"
OUT=$(sync --table simple --table sparse --table wide --table bulk --parallel 4)
eq "并行 4 表全部成功" "4" "$(echo "$OUT" | sed -n 's/.*成功 \([0-9]*\) 表.*/\1/p' | tail -1)"
eq "bulk 并行下仍取满" "10000" "$(dst_mysql "SELECT COUNT(*) FROM bulk")"

echo
echo "== 15. 同实例跨库同步 =="
docker exec rowbridge-src mysql -uroot -proot123 -e "DROP DATABASE IF EXISTS db_c; CREATE DATABASE db_c" 2>/dev/null
OUT=$($BIN --src-host 127.0.0.1 --src-port 3307 --src-user root --src-pass root123 \
          --dst-host 127.0.0.1 --dst-port 3307 --dst-user root --dst-pass root123 \
          sync --from db_a --to db_c --table simple 2>&1)
eq "跨库同步成功" "5" "$(src_mysql "SELECT COUNT(*) FROM simple" db_c)"

echo
echo "== 16. 中断后续跑：行数与内容均与源表一致 =="
src_mysql "DROP TABLE IF EXISTS huge;
CREATE TABLE huge (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT, v VARCHAR(200) NOT NULL, PRIMARY KEY(id)) ENGINE=InnoDB;
SET SESSION cte_max_recursion_depth=1000000;
INSERT INTO huge (v) WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM s WHERE n<400000)
  SELECT CONCAT(REPEAT('x',150), n) FROM s;" >/dev/null
dst_exec "DROP TABLE IF EXISTS db_b.huge"

$BIN $COMMON sync --from db_a --to db_b --table huge --batch 500 --no-adaptive >/dev/null 2>&1 &
CLI_PID=$!
# 等它真的开始写了再中断，否则小表可能已经跑完
for _ in $(seq 1 100); do
  n=$(dst_mysql "SELECT COUNT(*) FROM huge" 2>/dev/null || echo 0)
  [ "${n:-0}" -gt 2000 ] && break
  sleep 0.1
done
kill -TERM $CLI_PID 2>/dev/null
wait $CLI_PID 2>/dev/null

partial=$(dst_mysql "SELECT COUNT(*) FROM huge")
if [ "$partial" -gt 0 ] && [ "$partial" -lt 400000 ]; then
  ok "中断时已写入部分数据（$partial 行）"
else
  bad "中断时已写入部分数据" "0 < n < 400000" "$partial"
fi

sync --table huge --batch 1000 --cursor resume >/dev/null
eq "续跑后行数补齐" "400000" "$(dst_mysql "SELECT COUNT(*) FROM huge")"
src_md5=$(src_mysql "SELECT MD5(GROUP_CONCAT(MD5(CONCAT(id,'#',v)) ORDER BY id SEPARATOR '')) FROM huge")
dst_md5=$(dst_mysql "SELECT MD5(GROUP_CONCAT(MD5(CONCAT(id,'#',v)) ORDER BY id SEPARATOR '')) FROM huge")
eq "续跑后内容与源表一致" "$src_md5" "$dst_md5"

echo
echo "== 17. 逐表游标：各表用各自区间，互不影响 =="
dst_exec "DROP DATABASE IF EXISTS db_b; CREATE DATABASE db_b"
# 用 Rust 集成测试直接驱动引擎，给两张表设不同区间，
# 断言各自只同步自己那段（CLI 只暴露全局游标，覆盖不到这里）。
if cargo test -p rowbridge-core --test per_table_cursor -- --nocapture 2>&1 | grep -q "test result: ok"; then
  ok "两张表各用自己的游标区间（集成测试）"
else
  bad "两张表各用自己的游标区间" "test result: ok" "见 cargo test 输出"
fi

echo
echo "==================================="
printf "通过 \033[32m%d\033[0m，失败 \033[31m%d\033[0m\n" "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
