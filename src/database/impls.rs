use super::*;

use serenity::model::prelude::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use roblox::*;

impl <T : FromSql> FromSqlRow for T {
    fn from_sql_row(row: Row) -> Result<T> {
        ensure!(row.len() == 1, "Row has the wrong number of values!");
        row.get(0)
    }
}
impl FromSqlRow for () {
    fn from_sql_row(row: Row) -> Result<()> {
        ensure!(row.len() == 0, "Row has the wrong number of values!");
        Ok(())
    }
}

impl <T : ToSql> ToSqlArgs for T {
    fn to_sql_args<R, F>(&self, f: F) -> Result<R> where F: FnOnce(&[&RusqliteToSql]) -> Result<R> {
        f(&[&ToSqlWrapper(self)])
    }
}
impl ToSqlArgs for () {
    fn to_sql_args<R, F>(&self, f: F) -> Result<R> where F: FnOnce(&[&RusqliteToSql]) -> Result<R> {
        f(&[])
    }
}

macro_rules! from_rusqlite {
    ($($ty:ty),* $(,)*) => {
        $(
            impl FromSql for $ty {
                fn from_sql(value: ValueRef) -> Result<Self> {
                    Ok(<$ty as RusqliteFromSql>::column_result(value)?)
                }
            }
        )*
    }
}
from_rusqlite! {
    i8, i16, i32, i64, isize,
    u8, u16, u32,
    f64, bool, String, Vec<u8>,
}
impl <T: FromSql> FromSql for Option<T> {
    fn from_sql(value: ValueRef) -> Result<Self> {
        match value {
            ValueRef::Null => Ok(None),
            _ => T::from_sql(value).map(Some),
        }
    }
}
impl FromSql for u64 {
    fn from_sql(value: ValueRef) -> Result<Self> {
        <i64 as FromSql>::from_sql(value).map(|x| x as u64)
    }
}
impl FromSql for usize {
    fn from_sql(value: ValueRef) -> Result<Self> {
        <isize as FromSql>::from_sql(value).map(|x| x as usize)
    }
}
impl FromSql for UserId {
    fn from_sql(value: ValueRef) -> Result<Self> {
        Ok(UserId(u64::from_sql(value)?))
    }
}
impl FromSql for GuildId {
    fn from_sql(value: ValueRef) -> Result<Self> {
        Ok(GuildId(u64::from_sql(value)?))
    }
}
impl FromSql for RobloxUserID {
    fn from_sql(value: ValueRef) -> Result<Self> {
        Ok(RobloxUserID(u64::from_sql(value)?))
    }
}
impl FromSql for SystemTime {
    fn from_sql(value: ValueRef) -> Result<Self> {
        Ok(UNIX_EPOCH + Duration::from_secs(u64::from_sql(value)?))
    }
}

macro_rules! to_rusqlite {
    ($($ty:ty),* $(,)*) => {
        $(
            impl ToSql for $ty {
                fn to_sql(&self) -> Result<ToSqlOutput> {
                    Ok(<$ty as RusqliteToSql>::to_sql(self)?)
                }
            }
        )*
    }
}
to_rusqlite! {
    i8, i16, i32, i64, isize,
    u8, u16, u32,
    f64, bool, String, str, Vec<u8>, [u8],
}
impl <T: ToSql> ToSql for Option<T> {
    fn to_sql(&self) -> Result<ToSqlOutput> {
        match *self {
            None => Ok(ToSqlOutput::Owned(Value::Null)),
            Some(ref t) => t.to_sql(),
        }
    }
}
impl <'a, T: ToSql + ?Sized> ToSql for &'a T {
    fn to_sql(&self) -> Result<ToSqlOutput> {
        (*self).to_sql()
    }
}
impl ToSql for u64 {
    fn to_sql(&self) -> Result<ToSqlOutput> {
        Ok(Value::Integer(*self as i64).into())
    }
}
impl ToSql for usize {
    fn to_sql(&self) -> Result<ToSqlOutput> {
        Ok(Value::Integer(*self as i64).into())
    }
}
impl ToSql for UserId {
    fn to_sql(&self) -> Result<ToSqlOutput> {
        self.0.to_sql()
    }
}
impl ToSql for GuildId {
    fn to_sql(&self) -> Result<ToSqlOutput> {
        self.0.to_sql()
    }
}
impl ToSql for RobloxUserID {
    fn to_sql(&self) -> Result<ToSqlOutput> {
        self.0.to_sql()
    }
}
impl ToSql for SystemTime {
    fn to_sql(&self) -> Result<ToSqlOutput> {
        Ok(Value::Integer(self.duration_since(UNIX_EPOCH)?.as_secs() as i64).into())
    }
}

macro_rules! tuple_impls {
    (@one $i:ident) => { 1 };
    (@count $($i:ident)*) => { 0 $(+ tuple_impls!(@one $i))* };
    ($rest_ty:ident $rest_var:ident,) => { };
    ($first_ty:ident $first_var:ident, $($rest_ty:ident $rest_var:ident,)*) => {
        #[allow(non_camel_case_types)]
        impl <$first_ty: FromSql $(, $rest_ty: FromSql)*>
            FromSqlRow for ($first_ty $(, $rest_ty)*) {

            #[allow(unused_assignments)]
            fn from_sql_row(row: Row) -> Result<Self> {
                ensure!(row.len() == 1 + tuple_impls!(@count $($rest_ty)*),
                        "Row has the wrong number of values!");

                let $first_var: $first_ty = row.get(0)?;
                let mut i = 1;
                $(
                    let $rest_var: $rest_ty = row.get(i)?;
                    i += 1;
                )*
                Ok(($first_var $(, $rest_var)*))
            }
        }

        #[allow(non_camel_case_types)]
        impl <$first_ty: ToSql $(, $rest_ty: ToSql)*> ToSqlArgs for ($first_ty $(, $rest_ty)*) {
            fn to_sql_args<R, F>(
                &self, f: F
            ) -> Result<R> where F: FnOnce(&[&RusqliteToSql]) -> Result<R> {
                let &(ref $first_var $(, ref $rest_var)*) = self;
                f(&[&ToSqlWrapper($first_var) $(, &ToSqlWrapper($rest_var))*])
            }
        }
        tuple_impls!($($rest_ty $rest_var,)*);
    };
}
tuple_impls! {
    Ty_01 var_01, Ty_02 var_02, Ty_03 var_03, Ty_04 var_04, Ty_05 var_05, Ty_06 var_06,
    Ty_07 var_07, Ty_08 var_08, Ty_09 var_09, Ty_10 var_10, Ty_11 var_11, Ty_12 var_12,
    Ty_13 var_13, Ty_14 var_14, Ty_15 var_15, Ty_16 var_16, Ty_17 var_17, Ty_18 var_18,
    Ty_19 var_19, Ty_20 var_20,
}