-- Global glossary database — schema v3.
-- 专名别名功能彻底移除（用户指示）：删除 name_aliases 表及其索引。
-- 别名历史数据删除为接受行为；names 表本身不含别名列，无需重建。

DROP INDEX IF EXISTS idx_name_aliases_form;
DROP TABLE name_aliases;
