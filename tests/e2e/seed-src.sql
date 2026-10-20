-- RowBridge e2e 测试数据
-- 每个表对应一个边界场景，见 docs/DESIGN.md §12

DROP DATABASE IF EXISTS db_a;
CREATE DATABASE db_a DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;
USE db_a;

-- 1. 普通表，主键连续
CREATE TABLE simple (
  id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  name VARCHAR(64) NOT NULL,
  score INT DEFAULT NULL,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
INSERT INTO simple (id, name, score) VALUES
  (1,'alice',10),(2,'bob',20),(3,'carol',30),(4,'dave',40),(5,'eve',50);

-- 2. 主键有大量空洞
CREATE TABLE sparse (
  id BIGINT NOT NULL,
  v VARCHAR(32),
  PRIMARY KEY (id)
) ENGINE=InnoDB;
INSERT INTO sparse (id, v) VALUES (1,'a'),(100,'b'),(100000,'c'),(1000000,'d');

-- 3. 主键含 0 和负数
CREATE TABLE signed_pk (
  id INT NOT NULL,
  v VARCHAR(32),
  PRIMARY KEY (id)
) ENGINE=InnoDB;
INSERT INTO signed_pk (id, v) VALUES (-100,'neg'),(-1,'near'),(0,'zero'),(1,'one'),(50,'pos');

-- 4. bigint unsigned 接近 u64::MAX
CREATE TABLE big_unsigned (
  id BIGINT UNSIGNED NOT NULL,
  v VARCHAR(32),
  PRIMARY KEY (id)
) ENGINE=InnoDB;
INSERT INTO big_unsigned (id, v) VALUES
  (9223372036854775807,'i64max'),
  (9223372036854775808,'above'),
  (18446744073709551614,'nearly max'),
  (18446744073709551615,'u64max');

-- 5. 空表
CREATE TABLE empty_t (
  id BIGINT NOT NULL,
  v VARCHAR(32),
  PRIMARY KEY (id)
) ENGINE=InnoDB;

-- 6. 复合主键（不支持）
CREATE TABLE composite_pk (
  id INT NOT NULL,
  ts INT NOT NULL,
  v VARCHAR(32),
  PRIMARY KEY (id, ts)
) ENGINE=InnoDB;
INSERT INTO composite_pk VALUES (1,1,'x');

-- 7. 无主键（不支持）
CREATE TABLE no_pk (
  a INT,
  b VARCHAR(32)
) ENGINE=InnoDB;
INSERT INTO no_pk VALUES (1,'x');

-- 8. 字符串主键（不支持）
CREATE TABLE str_pk (
  id VARCHAR(36) NOT NULL,
  v INT,
  PRIMARY KEY (id)
) ENGINE=InnoDB;
INSERT INTO str_pk VALUES ('abc',1);

-- 9. 各种数据类型
CREATE TABLE types_all (
  id INT NOT NULL,
  c_tinyint TINYINT,
  c_bool TINYINT(1),
  c_smallint SMALLINT,
  c_mediumint MEDIUMINT,
  c_int INT,
  c_bigint BIGINT,
  c_ubigint BIGINT UNSIGNED,
  c_decimal DECIMAL(20,6),
  c_float FLOAT,
  c_double DOUBLE,
  c_bit BIT(16),
  c_char CHAR(10),
  c_varchar VARCHAR(255),
  c_text TEXT,
  c_mediumtext MEDIUMTEXT,
  c_blob BLOB,
  c_json JSON,
  c_date DATE,
  c_time TIME(6),
  c_datetime DATETIME(6),
  c_timestamp TIMESTAMP NULL,
  c_year YEAR,
  c_enum ENUM('a','b','c'),
  c_set SET('x','y','z'),
  c_binary BINARY(8),
  c_varbinary VARBINARY(64),
  c_geometry GEOMETRY,
  PRIMARY KEY (id)
) ENGINE=InnoDB;
INSERT INTO types_all VALUES (
  1, -128, 1, -32768, -8388608, -2147483648, -9223372036854775808,
  18446744073709551615, 12345678901234.567890, 3.14159, 2.718281828459045,
  b'1010101010101010', 'chars', 'varchars', 'some text', REPEAT('m', 100000),
  UNHEX('DEADBEEF'), '{"k":"v","n":[1,2,3]}',
  '2024-02-29', '-838:59:59.000000', '2024-06-15 12:34:56.123456',
  '2024-06-15 12:34:56', 2024, 'b', 'x,z', UNHEX('0011223344556677'),
  UNHEX('CAFE'), ST_GeomFromText('POINT(1 2)')
);

-- 10. NULL 密集
CREATE TABLE nulls (
  id INT NOT NULL,
  a VARCHAR(32) DEFAULT NULL,
  b INT DEFAULT NULL,
  c DATETIME DEFAULT NULL,
  PRIMARY KEY (id)
) ENGINE=InnoDB;
INSERT INTO nulls (id,a,b,c) VALUES (1,NULL,NULL,NULL),(2,'x',NULL,NULL);

-- 11. 零日期（需要 sql_mode 放宽）
CREATE TABLE zero_date (
  id INT NOT NULL,
  d DATETIME,
  PRIMARY KEY (id)
) ENGINE=InnoDB;
SET SESSION sql_mode='';
INSERT INTO zero_date VALUES (1,'0000-00-00 00:00:00');

-- 12. 带外键（建表时需剥离）
CREATE TABLE parent_t (
  id INT NOT NULL,
  v VARCHAR(32),
  PRIMARY KEY (id)
) ENGINE=InnoDB;
INSERT INTO parent_t VALUES (1,'p1'),(2,'p2');
CREATE TABLE child_t (
  id INT NOT NULL,
  parent_id INT,
  v VARCHAR(32),
  PRIMARY KEY (id),
  CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES parent_t (id)
) ENGINE=InnoDB;
INSERT INTO child_t VALUES (1,1,'c1'),(2,2,'c2');

-- 13. 生成列（应被跳过）
CREATE TABLE generated_col (
  id INT NOT NULL,
  price DECIMAL(10,2) NOT NULL,
  qty INT NOT NULL,
  total DECIMAL(20,2) AS (price * qty) STORED,
  untaxed DECIMAL(20,2) AS (price * qty * 0.9) VIRTUAL,
  PRIMARY KEY (id)
) ENGINE=InnoDB;
INSERT INTO generated_col (id, price, qty) VALUES (1, 10.00, 3), (2, 5.50, 4);

-- 14. 宽表：100 列，用来触发占位符上限
CREATE TABLE wide (
  id INT NOT NULL,
  c001 INT, c002 INT, c003 INT, c004 INT, c005 INT, c006 INT, c007 INT, c008 INT, c009 INT, c010 INT,
  c011 INT, c012 INT, c013 INT, c014 INT, c015 INT, c016 INT, c017 INT, c018 INT, c019 INT, c020 INT,
  c021 INT, c022 INT, c023 INT, c024 INT, c025 INT, c026 INT, c027 INT, c028 INT, c029 INT, c030 INT,
  c031 INT, c032 INT, c033 INT, c034 INT, c035 INT, c036 INT, c037 INT, c038 INT, c039 INT, c040 INT,
  c041 INT, c042 INT, c043 INT, c044 INT, c045 INT, c046 INT, c047 INT, c048 INT, c049 INT, c050 INT,
  c051 INT, c052 INT, c053 INT, c054 INT, c055 INT, c056 INT, c057 INT, c058 INT, c059 INT, c060 INT,
  c061 INT, c062 INT, c063 INT, c064 INT, c065 INT, c066 INT, c067 INT, c068 INT, c069 INT, c070 INT,
  c071 INT, c072 INT, c073 INT, c074 INT, c075 INT, c076 INT, c077 INT, c078 INT, c079 INT, c080 INT,
  c081 INT, c082 INT, c083 INT, c084 INT, c085 INT, c086 INT, c087 INT, c088 INT, c089 INT, c090 INT,
  c091 INT, c092 INT, c093 INT, c094 INT, c095 INT, c096 INT, c097 INT, c098 INT, c099 INT, c100 INT,
  PRIMARY KEY (id)
) ENGINE=InnoDB;
INSERT INTO wide (id, c001, c050, c100)
SELECT seq, seq, seq*2, seq*3 FROM (
  SELECT 1 seq UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5
) t;

-- 15. 大字段：单行 ~2MB BLOB
CREATE TABLE big_blob (
  id INT NOT NULL,
  payload LONGBLOB,
  PRIMARY KEY (id)
) ENGINE=InnoDB;
INSERT INTO big_blob VALUES (1, REPEAT(UNHEX('AB'), 1024*1024));
INSERT INTO big_blob VALUES (2, REPEAT(UNHEX('CD'), 1024*1024));

-- 16. 批量数据表：用于验证分页不丢行
CREATE TABLE bulk (
  id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
  v VARCHAR(64) NOT NULL,
  n INT NOT NULL,
  PRIMARY KEY (id)
) ENGINE=InnoDB;
SET SESSION cte_max_recursion_depth = 100000;
INSERT INTO bulk (v, n)
WITH RECURSIVE seqs(seq) AS (
  SELECT 1 UNION ALL SELECT seq+1 FROM seqs WHERE seq < 10000
)
SELECT CONCAT('row_', seq), seq FROM seqs;
